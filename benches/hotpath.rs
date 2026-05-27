//! Internal hot-path cost calibration benchmark.

mod bench_runtime;

use async_trait::async_trait;
use bench_runtime::BenchRuntime;
use bytes::Bytes;
use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use futures::channel::mpsc;
use futures::{AsyncWrite, Stream, StreamExt};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::convert::TryFrom;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use zeromq::{
    __bench::{self, BenchPubFanoutBackend, BenchRoundRobinBackend, Message},
    prelude::*,
    SocketType, Transport, ZmqMessage, ZmqResult,
};

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

struct ReadyStream {
    next: usize,
}

impl Stream for ReadyStream {
    type Item = usize;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let item = self.next;
        self.next += 1;
        Poll::Ready(Some(item))
    }
}

#[derive(Default)]
struct CountingWrite {
    bytes_written: usize,
    flushes: usize,
}

impl AsyncWrite for CountingWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.bytes_written += buf.len();
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.flushes += 1;
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
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

    let mut group = c.benchmark_group("hotpath/message_construct/from_vec_u8");
    bench_runtime::configure_group(&mut group);
    for &msg_size in MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            let payload = vec![0xA6; s];
            b.iter(|| black_box(ZmqMessage::from(payload.clone())));
        });
    }
    group.finish();

    let mut group = c.benchmark_group("hotpath/message_construct/from_string");
    bench_runtime::configure_group(&mut group);
    for &msg_size in MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            let payload = "x".repeat(s);
            b.iter(|| black_box(ZmqMessage::from(payload.clone())));
        });
    }
    group.finish();

    let mut group = c.benchmark_group("hotpath/message_construct/from_str");
    bench_runtime::configure_group(&mut group);
    for &msg_size in MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            let payload = "x".repeat(s);
            b.iter(|| black_box(ZmqMessage::from(black_box(payload.as_str()))));
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

fn bench_message_accessors(c: &mut Criterion) {
    let mut group = c.benchmark_group("hotpath/message_accessors");
    bench_runtime::configure_group(&mut group);

    let one_frame = ZmqMessage::from(Bytes::from_static(b"payload"));
    let multipart = ZmqMessage::try_from(vec![
        Bytes::from_static(b"identity"),
        Bytes::from_static(b""),
        Bytes::from_static(b"payload"),
    ])
    .expect("multipart message");

    group.bench_function("len/one_frame", |b| {
        b.iter(|| black_box(one_frame.len()));
    });
    group.bench_function("is_empty/one_frame", |b| {
        b.iter(|| black_box(one_frame.is_empty()));
    });
    group.bench_function("get_first/one_frame", |b| {
        b.iter(|| black_box(one_frame.get(0)));
    });
    group.bench_function("iter_total_len/one_frame", |b| {
        b.iter(|| black_box(one_frame.iter().map(Bytes::len).sum::<usize>()));
    });

    group.bench_function("len/multipart", |b| {
        b.iter(|| black_box(multipart.len()));
    });
    group.bench_function("get_last/multipart", |b| {
        b.iter(|| black_box(multipart.get(2)));
    });
    group.bench_function("iter_total_len/multipart", |b| {
        b.iter(|| black_box(multipart.iter().map(Bytes::len).sum::<usize>()));
    });

    group.finish();
}

fn bench_message_mutation_and_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("hotpath/message_mutation_and_conversion");
    bench_runtime::configure_group(&mut group);

    let payload_bytes = payload(64, 0xB1);
    let second_payload = payload(64, 0xB2);
    let multipart_frames = vec![
        Bytes::from_static(b"identity"),
        Bytes::from_static(b""),
        payload_bytes.clone(),
    ];
    let prefix = ZmqMessage::try_from(vec![Bytes::from_static(b"prefix")]).expect("prefix");

    group.bench_function("push_back", |b| {
        b.iter(|| {
            let mut message = ZmqMessage::from(payload_bytes.clone());
            message.push_back(second_payload.clone());
            black_box(message);
        });
    });
    group.bench_function("push_front", |b| {
        b.iter(|| {
            let mut message = ZmqMessage::from(payload_bytes.clone());
            message.push_front(second_payload.clone());
            black_box(message);
        });
    });
    group.bench_function("pop_front", |b| {
        b.iter(|| {
            let mut message = ZmqMessage::try_from(multipart_frames.clone()).expect("multipart");
            black_box(__bench::message_pop_front(&mut message));
            black_box(message);
        });
    });
    group.bench_function("prepend", |b| {
        b.iter(|| {
            let mut message = ZmqMessage::from(payload_bytes.clone());
            message.prepend(&prefix);
            black_box(message);
        });
    });
    group.bench_function("split_off", |b| {
        b.iter(|| {
            let mut message = ZmqMessage::try_from(multipart_frames.clone()).expect("multipart");
            black_box(message.split_off(1));
            black_box(message);
        });
    });
    group.bench_function("try_from_vec_bytes", |b| {
        b.iter(|| black_box(ZmqMessage::try_from(multipart_frames.clone()).expect("multipart")));
    });
    group.bench_function("try_from_vecdeque_bytes", |b| {
        b.iter(|| {
            let frames: VecDeque<Bytes> = multipart_frames.clone().into();
            black_box(ZmqMessage::try_from(frames).expect("multipart"));
        });
    });
    group.bench_function("try_into_vec_u8", |b| {
        b.iter(|| {
            let message = ZmqMessage::from(payload_bytes.clone());
            black_box(Vec::<u8>::try_from(message).expect("single-frame bytes"));
        });
    });
    group.bench_function("try_into_string", |b| {
        b.iter(|| {
            let message = ZmqMessage::from("payload");
            black_box(String::try_from(message).expect("single-frame string"));
        });
    });

    group.finish();
}

fn bench_protocol_conversions(c: &mut Criterion) {
    let mut group = c.benchmark_group("hotpath/protocol_conversions");
    bench_runtime::configure_group(&mut group);

    group.bench_function("socket_type/as_str", |b| {
        b.iter(|| black_box(SocketType::DEALER.as_str()));
    });
    group.bench_function("socket_type/compatible", |b| {
        b.iter(|| black_box(SocketType::DEALER.compatible(SocketType::ROUTER)));
    });
    group.bench_function("socket_type/try_from_bytes", |b| {
        b.iter(|| black_box(SocketType::try_from(black_box(&b"DEALER"[..])).expect("socket type")));
    });
    group.bench_function("transport/as_str", |b| {
        b.iter(|| black_box(Transport::Tcp.as_str()));
    });
    group.bench_function("transport/try_from_str", |b| {
        b.iter(|| black_box(Transport::try_from(black_box("tcp")).expect("transport")));
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

fn bench_fair_queue_poll_next(c: &mut Criterion) {
    let rt = build_rt();
    let mut group = c.benchmark_group("hotpath/fair_queue/poll_next_ready");
    bench_runtime::configure_group(&mut group);

    for stream_count in [1_usize, 4, 16] {
        group.bench_with_input(
            BenchmarkId::from_parameter(stream_count),
            &stream_count,
            |b, &streams| {
                let mut queue: __bench::FairQueue<ReadyStream, usize> = __bench::fair_queue(false);
                for key in 0..streams {
                    __bench::fair_queue_insert(&mut queue, key, ReadyStream { next: 0 });
                }

                b.iter(|| {
                    let item = rt.block_on(queue.next()).expect("ready fair queue item");
                    black_box(item);
                });
            },
        );
    }

    group.finish();
}

fn bench_write_message_queue(c: &mut Criterion) {
    let rt = build_rt();
    let mut group = c.benchmark_group("hotpath/write_queue/write_message_queue");
    bench_runtime::configure_group(&mut group);

    for batch_size in [1_usize, 16, 128] {
        for &msg_size in &[64_usize, 1024] {
            group.throughput(Throughput::Bytes((batch_size * msg_size) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("batch={batch_size}"), msg_size),
                &(batch_size, msg_size),
                |b, &(batch, size)| {
                    let payload = payload(size, 0x91);
                    b.iter_batched(
                        || {
                            let (mut sender, receiver) = mpsc::channel(batch);
                            for _ in 0..batch {
                                sender
                                    .try_send(Message::Message(ZmqMessage::from(payload.clone())))
                                    .expect("prefill write queue");
                            }
                            drop(sender);
                            (receiver, CountingWrite::default())
                        },
                        |(receiver, writer)| {
                            rt.block_on(__bench::write_message_queue_to_writer(receiver, writer))
                                .expect("write queued messages");
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

fn bench_backend_round_robin(c: &mut Criterion) {
    let rt = build_rt();
    let mut group = c.benchmark_group("hotpath/backend/send_round_robin");
    bench_runtime::configure_group(&mut group);

    for peer_count in [1_usize, 4, 16] {
        group.bench_with_input(
            BenchmarkId::from_parameter(peer_count),
            &peer_count,
            |b, &peers| {
                let mut backend = rt.block_on(BenchRoundRobinBackend::new(
                    peers,
                    BENCH_PEER_SEND_QUEUE_CAPACITY,
                ));
                let payload = payload(64, 0x92);

                b.iter(|| {
                    rt.block_on(async {
                        backend
                            .send_round_robin(Message::Message(ZmqMessage::from(payload.clone())))
                            .await
                            .expect("round-robin send");
                    });
                    let delivered = backend.drain_ready();
                    assert_eq!(1, delivered);
                    black_box(delivered);
                });
            },
        );
    }

    group.finish();
}

fn bench_pub_fanout_backend(c: &mut Criterion) {
    let rt = build_rt();
    let mut group = c.benchmark_group("hotpath/pub_fanout_backend/fanout_message");
    bench_runtime::configure_group(&mut group);

    for subscriber_count in [1_usize, 8, 32] {
        for &msg_size in &[64_usize, 256] {
            group.throughput(Throughput::Bytes((subscriber_count * msg_size) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("subs={subscriber_count}"), msg_size),
                &(subscriber_count, msg_size),
                |b, &(subscribers, size)| {
                    let mut backend =
                        rt.block_on(BenchPubFanoutBackend::new(subscribers, vec![0xFA]));
                    let payload = Bytes::from(vec![0xFA; size]);

                    b.iter(|| {
                        rt.block_on(backend.fanout_message(ZmqMessage::from(payload.clone())));
                        let delivered = backend.drain_ready();
                        assert_eq!(subscribers, delivered);
                        black_box(delivered);
                    });
                },
            );
        }
    }

    group.finish();
}

fn bench_pub_fanout_backend_matches_all(c: &mut Criterion) {
    let rt = build_rt();
    let mut group = c.benchmark_group("hotpath/pub_fanout_backend/fanout_message_matches_all");
    bench_runtime::configure_group(&mut group);

    for subscriber_count in [1_usize, 8, 32] {
        for &msg_size in &[64_usize, 256] {
            group.throughput(Throughput::Bytes((subscriber_count * msg_size) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("subs={subscriber_count}"), msg_size),
                &(subscriber_count, msg_size),
                |b, &(subscribers, size)| {
                    let mut backend = rt.block_on(BenchPubFanoutBackend::new(subscribers, vec![]));
                    let payload = Bytes::from(vec![0xFA; size]);

                    b.iter(|| {
                        rt.block_on(backend.fanout_message(ZmqMessage::from(payload.clone())));
                        let delivered = backend.drain_ready();
                        assert_eq!(subscribers, delivered);
                        black_box(delivered);
                    });
                },
            );
        }
    }

    group.finish();
}

fn pub_subscriptions(subscription_count: usize, matching: bool) -> Vec<Vec<u8>> {
    let mut subscriptions = Vec::with_capacity(subscription_count);
    for index in 0..subscription_count {
        subscriptions.push(vec![0x10 + (index % 128) as u8]);
    }
    if matching {
        let last = subscriptions
            .last_mut()
            .expect("subscription_count must be non-zero");
        *last = vec![0xFA];
    }
    subscriptions
}

fn bench_pub_fanout_backend_many_subscriptions(c: &mut Criterion) {
    let rt = build_rt();
    let mut group =
        c.benchmark_group("hotpath/pub_fanout_backend/fanout_message_many_subscriptions");
    bench_runtime::configure_group(&mut group);

    for subscriber_count in [1_usize, 8] {
        for subscription_count in [8_usize, 64] {
            for matching in [true, false] {
                let scenario = if matching { "match_last" } else { "miss" };
                group.throughput(Throughput::Bytes((subscriber_count * 64) as u64));
                group.bench_with_input(
                    BenchmarkId::new(
                        format!("subs={subscriber_count}/filters={subscription_count}"),
                        scenario,
                    ),
                    &(subscriber_count, subscription_count, matching),
                    |b, &(subscribers, filters, matching)| {
                        let subscriptions = pub_subscriptions(filters, matching);
                        let mut backend =
                            rt.block_on(BenchPubFanoutBackend::new_with_subscriptions(
                                subscribers,
                                subscriptions,
                            ));
                        let payload = Bytes::from(vec![0xFA; 64]);
                        let expected_delivered = if matching { subscribers } else { 0 };

                        b.iter(|| {
                            rt.block_on(backend.fanout_message(ZmqMessage::from(payload.clone())));
                            let delivered = backend.drain_ready();
                            assert_eq!(expected_delivered, delivered);
                            black_box(delivered);
                        });
                    },
                );
            }
        }
    }

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
    bench_pub_fanout_backend_many_subscriptions,
    bench_pub_fanout_backend_matches_all,
    bench_pub_fanout_backend,
    bench_backend_round_robin,
    bench_write_message_queue,
    bench_fair_queue_poll_next,
    bench_backend_primitives,
    bench_protocol_conversions,
    bench_message_mutation_and_conversion,
    bench_message_accessors,
    bench_async_send_overhead,
    bench_runtime_overhead,
    bench_message_construct,
);
criterion_main!(benches);
