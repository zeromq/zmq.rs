//! Internal hot-path cost calibration benchmark.

mod bench_runtime;

use async_trait::async_trait;
use bench_runtime::BenchRuntime;
use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::channel::mpsc;
use parking_lot::Mutex;
use std::collections::VecDeque;

use zeromq::{__bench::Message, prelude::*, ZmqMessage, ZmqResult};

const MSG_SIZES: &[usize] = &[64, 256, 1024];
const BENCH_PEER_SEND_QUEUE_CAPACITY: usize = 100_000;

struct NoopInherentSender;

impl NoopInherentSender {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        black_box(message);
        Ok(())
    }
}

struct NoopTraitSender;

#[async_trait]
impl SocketSend for NoopTraitSender {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        black_box(message);
        Ok(())
    }
}

struct NestedNoopTraitSender;

#[async_trait]
impl SocketSend for NestedNoopTraitSender {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        nested_noop_send(message).await?;
        Ok(())
    }
}

async fn nested_noop_send(message: ZmqMessage) -> ZmqResult<()> {
    black_box(message);
    Ok(())
}

fn build_rt() -> BenchRuntime {
    BenchRuntime::new()
}

fn payload(size: usize, byte: u8) -> Bytes {
    Bytes::from(vec![byte; size])
}

fn bench_message_construct(c: &mut Criterion) {
    let mut group = c.benchmark_group("hotpath/message_construct/from_bytes");
    bench_runtime::configure_group(&mut group);
    for &msg_size in MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            let payload = payload(s, 0xA5);
            b.iter(|| black_box(ZmqMessage::from(payload.clone())));
        });
    }
    group.finish();

    let mut group = c.benchmark_group("hotpath/message_construct/vecdeque_single_frame");
    bench_runtime::configure_group(&mut group);
    let payload = payload(64, 0xA5);
    group.bench_function("with_capacity_push_back", |b| {
        b.iter(|| {
            let mut frames = VecDeque::with_capacity(1);
            frames.push_back(payload.clone());
            black_box(frames);
        });
    });
    group.bench_function("from_array", |b| {
        b.iter(|| black_box(VecDeque::from([payload.clone()])));
    });
    group.bench_function("vec_into", |b| {
        b.iter(|| {
            let frames: VecDeque<Bytes> = vec![payload.clone()].into();
            black_box(frames);
        });
    });
    group.finish();
}

fn bench_runtime_overhead(c: &mut Criterion) {
    let rt = build_rt();
    let mut group = c.benchmark_group("hotpath/runtime");
    bench_runtime::configure_group(&mut group);
    group.bench_function("block_on_noop", |b| {
        b.iter(|| rt.block_on(async { black_box(()) }));
    });
    group.finish();
}

fn bench_async_send_overhead(c: &mut Criterion) {
    let rt = build_rt();
    let mut group = c.benchmark_group("hotpath/async_send_overhead");
    bench_runtime::configure_group(&mut group);

    for &msg_size in MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));

        group.bench_with_input(
            BenchmarkId::new("inherent_async", msg_size),
            &msg_size,
            |b, &s| {
                let payload = payload(s, 0x71);
                let mut sender = NoopInherentSender;
                b.iter(|| {
                    rt.block_on(async {
                        sender
                            .send(ZmqMessage::from(payload.clone()))
                            .await
                            .expect("inherent async send");
                    });
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("async_trait", msg_size),
            &msg_size,
            |b, &s| {
                let payload = payload(s, 0x72);
                let mut sender = NoopTraitSender;
                b.iter(|| {
                    rt.block_on(async {
                        sender
                            .send(ZmqMessage::from(payload.clone()))
                            .await
                            .expect("async trait send");
                    });
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("async_trait_nested_await", msg_size),
            &msg_size,
            |b, &s| {
                let payload = payload(s, 0x73);
                let mut sender = NestedNoopTraitSender;
                b.iter(|| {
                    rt.block_on(async {
                        sender
                            .send(ZmqMessage::from(payload.clone()))
                            .await
                            .expect("nested async trait send");
                    });
                });
            },
        );
    }

    group.finish();
}

fn bench_backend_primitives(c: &mut Criterion) {
    let mut group = c.benchmark_group("hotpath/backend_primitives");
    bench_runtime::configure_group(&mut group);

    group.bench_function("parking_lot_mutex_lock", |b| {
        let mutex = Mutex::new(());
        b.iter(|| {
            let guard = mutex.lock();
            black_box(&*guard);
        });
    });

    group.bench_function("futures_mpsc_try_send_recv_unit", |b| {
        let (mut sender, mut receiver) = mpsc::channel(1);
        b.iter(|| {
            sender.try_send(()).expect("try_send unit");
            let _: () = receiver.try_recv().expect("try_recv unit");
            black_box(());
        });
    });

    group.bench_function("futures_mpsc_try_send_recv_message", |b| {
        let (mut sender, mut receiver) = mpsc::channel(1);
        let payload = payload(64, 0x5A);
        b.iter(|| {
            let message = Message::Message(ZmqMessage::from(payload.clone()));
            sender.try_send(message).expect("try_send message");
            black_box(receiver.try_recv().expect("try_recv message"));
        });
    });

    group.bench_function("futures_mpsc_try_send_recv_message_cap_100k", |b| {
        let (mut sender, mut receiver) = mpsc::channel(BENCH_PEER_SEND_QUEUE_CAPACITY);
        let payload = payload(64, 0x5A);
        b.iter(|| {
            let message = Message::Message(ZmqMessage::from(payload.clone()));
            sender.try_send(message).expect("try_send message");
            black_box(receiver.try_recv().expect("try_recv message"));
        });
    });

    group.bench_function("mutex_mpsc_try_send_recv_message", |b| {
        let (sender, mut receiver) = mpsc::channel(1);
        let sender = Mutex::new(sender);
        let payload = payload(64, 0x5A);
        b.iter(|| {
            let message = Message::Message(ZmqMessage::from(payload.clone()));
            {
                let mut sender = sender.lock();
                sender.try_send(message).expect("try_send message");
            }
            black_box(receiver.try_recv().expect("try_recv message"));
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_backend_primitives,
    bench_async_send_overhead,
    bench_runtime_overhead,
    bench_message_construct,
);
criterion_main!(benches);
