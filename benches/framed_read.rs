//! Framed-read microbenchmarks for receive-path chunk sizing.

#[allow(dead_code)]
mod bench_runtime;

use asynchronous_codec::{Encoder, FramedRead};
use bytes::{Bytes, BytesMut};
use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use futures::{executor::block_on, io::Cursor, Stream, StreamExt};

use zeromq::{
    __bench::{zmq_framed_read, Message, ZmqCodec, ZmqFramedRead},
    ZmqMessage,
};

const FRAME_SIZES: &[usize] = &[256, 4096, 65536];
const MESSAGE_COUNTS: &[usize] = &[1, 1024];

const GREETING_STUB: [u8; 64] = {
    let mut greeting = [0; 64];
    greeting[0] = 0xff;
    greeting[9] = 0x7f;
    greeting[10] = 3;
    greeting[11] = 1;
    greeting[12] = b'N';
    greeting[13] = b'U';
    greeting[14] = b'L';
    greeting[15] = b'L';
    greeting
};

fn encoded_stream(message_count: usize, frame_size: usize) -> Vec<u8> {
    let mut stream = BytesMut::new();
    stream.extend_from_slice(&GREETING_STUB);

    let payload = Bytes::from(vec![0xAB; frame_size]);
    let message = Message::Message(ZmqMessage::from(payload));
    let mut codec = ZmqCodec::new();

    for _ in 0..message_count {
        codec
            .encode(message.clone(), &mut stream)
            .expect("encode framed-read input");
    }

    stream.to_vec()
}

async fn drain_stream<S, E>(mut stream: S) -> usize
where
    S: Stream<Item = Result<Message, E>> + Unpin,
    E: std::fmt::Debug,
{
    let mut messages = 0;

    while let Some(item) = stream.next().await {
        if matches!(item.expect("decode framed-read input"), Message::Message(_)) {
            messages += 1;
        }
    }

    messages
}

fn bench_framed_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("framed_read/drain");
    bench_runtime::configure_group(&mut group);

    for &message_count in MESSAGE_COUNTS {
        for &frame_size in FRAME_SIZES {
            group.throughput(criterion::Throughput::Bytes(
                (message_count * frame_size) as u64,
            ));

            let input = encoded_stream(message_count, frame_size);
            group.bench_with_input(
                BenchmarkId::new(format!("default/messages={message_count}"), frame_size),
                &input,
                |b, input| {
                    b.iter_batched(
                        || Cursor::new(input.clone()),
                        |cursor| {
                            let reader = FramedRead::new(cursor, ZmqCodec::new());
                            let messages = block_on(drain_stream(reader));
                            assert_eq!(messages, message_count);
                            black_box(messages);
                        },
                        BatchSize::SmallInput,
                    );
                },
            );

            group.bench_with_input(
                BenchmarkId::new(format!("adaptive/messages={message_count}"), frame_size),
                &input,
                |b, input| {
                    b.iter_batched(
                        || Cursor::new(input.clone()),
                        |cursor| {
                            let reader: ZmqFramedRead = zmq_framed_read(cursor);
                            let messages = block_on(drain_stream(reader));
                            assert_eq!(messages, message_count);
                            black_box(messages);
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, bench_framed_read);
criterion_main!(benches);
