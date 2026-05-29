//! Fanout queue latency calibration for the PUB local fanout path.

mod bench_runtime;

use bench_runtime::BenchRuntime;
use bytes::Bytes;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use crossbeam_queue::ArrayQueue;
use futures::channel::mpsc;
use futures::StreamExt;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use zeromq::{__async_rt::task, ZmqMessage, ZmqResult};

const FANOUT_QUEUE_CAPACITY: usize = 100_000;
const THROUGHPUT_BATCH_MESSAGES: usize = 8_192;
const MSG_SIZES: &[usize] = &[64, 256, 1024];

struct ArrayFanoutQueue {
    queue: Arc<ArrayQueue<ZmqMessage>>,
    wake_sender: mpsc::UnboundedSender<()>,
    wake_pending: Arc<AtomicBool>,
}

impl ArrayFanoutQueue {
    fn publish(&self, message: ZmqMessage) -> ZmqResult<()> {
        if self.queue.push(message).is_err() {
            return Ok(());
        }

        if !self.wake_pending.load(Ordering::Acquire)
            && !self.wake_pending.swap(true, Ordering::AcqRel)
        {
            let _ = self.wake_sender.unbounded_send(());
        }

        Ok(())
    }
}

fn payload(size: usize, byte: u8) -> Bytes {
    Bytes::from(vec![byte; size])
}

fn build_array_queue(capacity: usize) -> (ArrayFanoutQueue, mpsc::UnboundedReceiver<()>) {
    let (wake_sender, wake_receiver) = mpsc::unbounded();
    let queue = ArrayFanoutQueue {
        queue: Arc::new(ArrayQueue::new(capacity)),
        wake_sender,
        wake_pending: Arc::new(AtomicBool::new(false)),
    };
    (queue, wake_receiver)
}

fn spawn_array_worker(
    queue: Arc<ArrayQueue<ZmqMessage>>,
    wake_pending: Arc<AtomicBool>,
    mut wake_receiver: mpsc::UnboundedReceiver<()>,
    ack_sender: mpsc::UnboundedSender<()>,
) {
    task::spawn(async move {
        while wake_receiver.next().await.is_some() {
            loop {
                while let Some(message) = queue.pop() {
                    black_box(message);
                    let _ = ack_sender.unbounded_send(());
                }

                wake_pending.store(false, Ordering::Release);
                if queue.is_empty() {
                    break;
                }

                wake_pending.store(true, Ordering::Release);
            }
        }
    });
}

fn spawn_mpsc_worker(
    mut receiver: mpsc::Receiver<ZmqMessage>,
    ack_sender: mpsc::UnboundedSender<()>,
) {
    task::spawn(async move {
        while let Some(message) = receiver.next().await {
            black_box(message);
            let _ = ack_sender.unbounded_send(());
        }
    });
}

fn bench_first_message_wake_latency(c: &mut Criterion) {
    let rt = BenchRuntime::new();
    let mut group = c.benchmark_group("hotpath/pub_fanout_queue/first_message_wake_latency");
    bench_runtime::configure_group(&mut group);

    for &msg_size in MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));

        group.bench_with_input(
            BenchmarkId::new("array_queue_wake_pending", msg_size),
            &msg_size,
            |b, &size| {
                let (queue, wake_receiver) = build_array_queue(FANOUT_QUEUE_CAPACITY);
                let (ack_sender, mut ack_receiver) = mpsc::unbounded();
                let payload = payload(size, 0xA1);

                rt.block_on(async {
                    spawn_array_worker(
                        queue.queue.clone(),
                        queue.wake_pending.clone(),
                        wake_receiver,
                        ack_sender,
                    );
                });

                b.iter(|| {
                    rt.block_on(async {
                        queue
                            .publish(ZmqMessage::from(payload.clone()))
                            .expect("array publish");
                        ack_receiver.next().await.expect("array worker ack");
                    });
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("bounded_futures_mpsc", msg_size),
            &msg_size,
            |b, &size| {
                let (mut sender, receiver) = mpsc::channel(FANOUT_QUEUE_CAPACITY);
                let (ack_sender, mut ack_receiver) = mpsc::unbounded();
                let payload = payload(size, 0xA2);

                rt.block_on(async {
                    spawn_mpsc_worker(receiver, ack_sender);
                });

                b.iter(|| {
                    rt.block_on(async {
                        sender
                            .try_send(ZmqMessage::from(payload.clone()))
                            .expect("mpsc publish");
                        ack_receiver.next().await.expect("mpsc worker ack");
                    });
                });
            },
        );
    }

    group.finish();
}

fn bench_enqueue_return_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("hotpath/pub_fanout_queue/enqueue_return_latency");
    bench_runtime::configure_group(&mut group);

    for &msg_size in MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));

        group.bench_with_input(
            BenchmarkId::new("array_queue_pending", msg_size),
            &msg_size,
            |b, &size| {
                let (queue, _wake_receiver) = build_array_queue(FANOUT_QUEUE_CAPACITY);
                let payload = payload(size, 0xB1);
                queue.wake_pending.store(true, Ordering::Relaxed);

                b.iter(|| {
                    queue
                        .publish(ZmqMessage::from(payload.clone()))
                        .expect("array publish");
                    black_box(queue.queue.pop().expect("array queued message"));
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("bounded_futures_mpsc", msg_size),
            &msg_size,
            |b, &size| {
                let (mut sender, mut receiver) = mpsc::channel(FANOUT_QUEUE_CAPACITY);
                let payload = payload(size, 0xB2);

                b.iter(|| {
                    sender
                        .try_send(ZmqMessage::from(payload.clone()))
                        .expect("mpsc publish");
                    black_box(receiver.try_recv().expect("mpsc queued message"));
                });
            },
        );
    }

    group.finish();
}

fn bench_full_drop_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("hotpath/pub_fanout_queue/full_drop_latency");
    bench_runtime::configure_group(&mut group);

    for &msg_size in MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));

        group.bench_with_input(
            BenchmarkId::new("array_queue", msg_size),
            &msg_size,
            |b, &size| {
                let (queue, _wake_receiver) = build_array_queue(1);
                let payload = payload(size, 0xC1);
                queue
                    .publish(ZmqMessage::from(payload.clone()))
                    .expect("prefill array queue");

                b.iter(|| {
                    queue
                        .publish(ZmqMessage::from(payload.clone()))
                        .expect("array drop publish");
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("bounded_futures_mpsc", msg_size),
            &msg_size,
            |b, &size| {
                let (mut sender, _receiver) = mpsc::channel(1);
                let payload = payload(size, 0xC2);
                let mut prefilled = 0;
                loop {
                    match sender.try_send(ZmqMessage::from(payload.clone())) {
                        Ok(()) => prefilled += 1,
                        Err(error) if error.is_full() => {
                            black_box(error.into_inner());
                            break;
                        }
                        Err(error) => panic!("unexpected mpsc prefill error: {error:?}"),
                    }
                }
                assert!(prefilled > 0);

                b.iter(
                    || match sender.try_send(ZmqMessage::from(payload.clone())) {
                        Ok(()) => panic!("mpsc queue unexpectedly accepted message"),
                        Err(error) if error.is_full() => {
                            black_box(error.into_inner());
                        }
                        Err(error) => panic!("unexpected mpsc error: {error:?}"),
                    },
                );
            },
        );
    }

    group.finish();
}

fn bench_sustained_throughput(c: &mut Criterion) {
    let rt = BenchRuntime::new();
    let mut group = c.benchmark_group("hotpath/pub_fanout_queue/sustained_throughput");
    bench_runtime::configure_group(&mut group);

    for &msg_size in MSG_SIZES {
        group.throughput(Throughput::Bytes(
            (msg_size * THROUGHPUT_BATCH_MESSAGES) as u64,
        ));

        group.bench_with_input(
            BenchmarkId::new("array_queue_wake_pending", msg_size),
            &msg_size,
            |b, &size| {
                let (queue, wake_receiver) = build_array_queue(FANOUT_QUEUE_CAPACITY);
                let (ack_sender, mut ack_receiver) = mpsc::unbounded();
                let payload = payload(size, 0xD1);

                rt.block_on(async {
                    spawn_array_worker(
                        queue.queue.clone(),
                        queue.wake_pending.clone(),
                        wake_receiver,
                        ack_sender,
                    );
                });

                b.iter_custom(|iters| {
                    let start = Instant::now();

                    rt.block_on(async {
                        for _ in 0..iters {
                            for _ in 0..THROUGHPUT_BATCH_MESSAGES {
                                queue
                                    .publish(ZmqMessage::from(payload.clone()))
                                    .expect("array publish");
                            }

                            for _ in 0..THROUGHPUT_BATCH_MESSAGES {
                                ack_receiver.next().await.expect("array worker ack");
                            }
                        }
                    });

                    start.elapsed()
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("bounded_futures_mpsc", msg_size),
            &msg_size,
            |b, &size| {
                let (mut sender, receiver) = mpsc::channel(FANOUT_QUEUE_CAPACITY);
                let (ack_sender, mut ack_receiver) = mpsc::unbounded();
                let payload = payload(size, 0xD2);

                rt.block_on(async {
                    spawn_mpsc_worker(receiver, ack_sender);
                });

                b.iter_custom(|iters| {
                    let start = Instant::now();

                    rt.block_on(async {
                        for _ in 0..iters {
                            for _ in 0..THROUGHPUT_BATCH_MESSAGES {
                                sender
                                    .try_send(ZmqMessage::from(payload.clone()))
                                    .expect("mpsc publish");
                            }

                            for _ in 0..THROUGHPUT_BATCH_MESSAGES {
                                ack_receiver.next().await.expect("mpsc worker ack");
                            }
                        }
                    });

                    start.elapsed()
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_first_message_wake_latency,
    bench_enqueue_return_latency,
    bench_full_drop_latency,
    bench_sustained_throughput,
);
criterion_main!(benches);
