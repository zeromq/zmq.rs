//! Pure codec microbenchmarks: no sockets, no I/O.

mod bench_runtime;

use asynchronous_codec::{Decoder, Encoder};
use bytes::{Bytes, BytesMut};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

use zeromq::{
    __bench::{Message, ZmqCodec},
    ZmqMessage,
};

const FRAME_SIZES: &[usize] = &[16, 256, 4096, 65536];
const MULTIPART_FRAME_COUNTS: &[usize] = &[1, 2, 8];

fn build_message(frame_count: usize, frame_size: usize) -> ZmqMessage {
    let mut m = ZmqMessage::from(Bytes::from(vec![0xAB; frame_size]));
    for _ in 1..frame_count {
        m.push_back(Bytes::from(vec![0xCD; frame_size]));
    }
    m
}

fn bench_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("codec/encode");
    bench_runtime::configure_group(&mut group);
    for &frames in MULTIPART_FRAME_COUNTS {
        for &size in FRAME_SIZES {
            let m = build_message(frames, size);
            let total_bytes = (frames * size) as u64;
            group.throughput(Throughput::Bytes(total_bytes));
            group.bench_with_input(
                BenchmarkId::new(format!("frames={frames}"), size),
                &(frames, size),
                |b, _| {
                    b.iter(|| {
                        let mut codec = ZmqCodec::new();
                        let mut dst = BytesMut::with_capacity(total_bytes as usize + 64);
                        codec
                            .encode(Message::Message(m.clone()), &mut dst)
                            .expect("encode");
                        black_box(dst);
                    });
                },
            );
        }
    }
    group.finish();
}

fn bench_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("codec/decode");
    bench_runtime::configure_group(&mut group);
    for &frames in MULTIPART_FRAME_COUNTS {
        for &size in FRAME_SIZES {
            let m = build_message(frames, size);
            let total_bytes = (frames * size) as u64;

            let mut encoded = BytesMut::new();
            ZmqCodec::new()
                .encode(Message::Message(m), &mut encoded)
                .expect("encode for decode bench");
            let encoded = encoded.freeze();

            group.throughput(Throughput::Bytes(total_bytes));
            group.bench_with_input(
                BenchmarkId::new(format!("frames={frames}"), size),
                &(frames, size),
                |b, _| {
                    b.iter(|| {
                        let mut codec = ZmqCodec::new();
                        let mut src = BytesMut::new();
                        src.extend_from_slice(&GREETING_STUB);
                        src.extend_from_slice(&encoded);
                        let _ = codec.decode(&mut src).expect("greeting");
                        let msg = codec.decode(&mut src).expect("decode").expect("message");
                        black_box(msg);
                    });
                },
            );
        }
    }
    group.finish();
}

const GREETING_STUB: [u8; 64] = {
    let mut g = [0; 64];
    g[0] = 0xff;
    g[9] = 0x7f;
    g[10] = 3;
    g[11] = 1;
    g[12] = b'N';
    g[13] = b'U';
    g[14] = b'L';
    g[15] = b'L';
    g
};

criterion_group!(benches, bench_encode, bench_decode);
criterion_main!(benches);
