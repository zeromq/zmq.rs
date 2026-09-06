use super::{bench_runtime, GREETING_STUB};
use asynchronous_codec::Encoder;
use bytes::{Bytes, BytesMut};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use futures::{executor::block_on, AsyncRead, StreamExt};
use std::collections::VecDeque;
use std::hint::black_box;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use zeromq::{
    __bench::{zmq_framed_read, Message, ZmqCodec, ZmqFramedRead},
    ZmqMessage,
};

#[derive(Clone)]
struct Pattern {
    wire: Bytes,
    boundaries: Vec<usize>,
    messages: usize,
}

impl Pattern {
    fn small_messages(burst_then_small: bool) -> Self {
        let mut wire = BytesMut::from(GREETING_STUB.as_slice());
        let mut codec = ZmqCodec::new();
        let message = Message::Message(ZmqMessage::from(Bytes::from(vec![7; 190])));
        for _ in 0..4096 {
            codec.encode(message.clone(), &mut wire).unwrap();
        }
        let boundaries = if burst_then_small {
            // 1024 burst messages, 2048 individually available messages, then another burst.
            (1024..=3072).map(|i| 64 + i * 192).collect()
        } else {
            Vec::new()
        };
        Self {
            wire: wire.freeze(),
            boundaries,
            messages: 4096,
        }
    }

    fn gapped_batches() -> Self {
        let mut pattern = Self::small_messages(false);
        pattern.boundaries = (1..=64).map(|i| 64 + i * 64 * 192).collect();
        pattern
    }

    fn sized_messages(
        frame_size: usize,
        count: usize,
        multipart: bool,
        boundary_each: bool,
    ) -> Self {
        let mut wire = BytesMut::from(GREETING_STUB.as_slice());
        let mut codec = ZmqCodec::new();
        let mut message = ZmqMessage::from(Bytes::from(vec![9; frame_size]));
        if multipart {
            message.push_back(Bytes::from_static(b"topic"));
            message.push_back(Bytes::from(vec![3; 16_042]));
        }
        let mut boundaries = Vec::new();
        for _ in 0..count {
            codec
                .encode(Message::Message(message.clone()), &mut wire)
                .unwrap();
            if boundary_each {
                boundaries.push(wire.len());
            }
        }
        Self {
            wire: wire.freeze(),
            boundaries,
            messages: count,
        }
    }
}

struct PatternReader {
    pattern: Pattern,
    position: usize,
    next_boundary: usize,
}

impl PatternReader {
    fn new(pattern: Pattern) -> Self {
        Self {
            pattern,
            position: 0,
            next_boundary: 0,
        }
    }
}

impl AsyncRead for PatternReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        while self
            .pattern
            .boundaries
            .get(self.next_boundary)
            .is_some_and(|&end| end <= self.position)
        {
            self.next_boundary += 1;
        }
        let available_end = self
            .pattern
            .boundaries
            .get(self.next_boundary)
            .copied()
            .unwrap_or(self.pattern.wire.len());
        let count = buffer.len().min(available_end - self.position);
        let end = self.position + count;
        buffer[..count].copy_from_slice(&self.pattern.wire[self.position..end]);
        self.position = end;
        Poll::Ready(Ok(count))
    }
}

#[derive(Debug, Default)]
struct ReadCounts {
    prepared_bytes: usize,
    read_calls: usize,
}

struct CountingReader {
    inner: PatternReader,
    counts: Arc<Mutex<ReadCounts>>,
}

impl AsyncRead for CountingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        {
            let mut counts = self.counts.lock().unwrap();
            counts.prepared_bytes += buffer.len();
            counts.read_calls += 1;
        }
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}

async fn drain_retained(mut reader: ZmqFramedRead) -> usize {
    let mut retained = VecDeque::with_capacity(65);
    let mut messages = 0;
    while let Some(item) = reader.next().await {
        if let Message::Message(message) = item.unwrap() {
            retained.push_back(message);
            if retained.len() > 64 {
                retained.pop_front();
            }
            messages += 1;
        }
    }
    black_box(retained);
    messages
}

pub fn bench_read_buffer_patterns(c: &mut Criterion) {
    let patterns = [
        ("burst_small_burst", Pattern::small_messages(true)),
        ("continuous_burst", Pattern::small_messages(false)),
        ("gapped_batches", Pattern::gapped_batches()),
        (
            "large_boundaries",
            Pattern::sized_messages(338_729, 32, false, true),
        ),
        (
            "multipart_boundaries",
            Pattern::sized_messages(338_729, 32, true, true),
        ),
        (
            "single_large",
            Pattern::sized_messages(338_729, 1, false, false),
        ),
        (
            "frame_65535_boundaries",
            Pattern::sized_messages(65_535, 64, false, true),
        ),
        (
            "frame_65536_boundaries",
            Pattern::sized_messages(65_536, 64, false, true),
        ),
        (
            "frame_65537_boundaries",
            Pattern::sized_messages(65_537, 64, false, true),
        ),
        (
            "frame_131071_boundaries",
            Pattern::sized_messages(131_071, 64, false, true),
        ),
        (
            "frame_131072_boundaries",
            Pattern::sized_messages(131_072, 64, false, true),
        ),
        (
            "frame_131073_boundaries",
            Pattern::sized_messages(131_073, 64, false, true),
        ),
        (
            "large_frames",
            Pattern::sized_messages(338_729, 32, false, false),
        ),
        (
            "multipart",
            Pattern::sized_messages(338_729, 32, true, false),
        ),
    ];
    let mut group = c.benchmark_group("framed_read/retained");
    bench_runtime::configure_group(&mut group);
    for (name, pattern) in patterns {
        group.throughput(Throughput::Bytes(pattern.wire.len() as u64));
        // Collect deterministic work counts separately; timed readers have no counters.
        let counts = Arc::new(Mutex::new(ReadCounts::default()));
        let counted = CountingReader {
            inner: PatternReader::new(pattern.clone()),
            counts: Arc::clone(&counts),
        };
        let reader = zmq_framed_read(counted);
        assert_eq!(block_on(drain_retained(reader)), pattern.messages);
        eprintln!("read_work {name}/default: {:?}", counts.lock().unwrap());
        group.bench_with_input(BenchmarkId::new(name, "default"), &pattern, |b, pattern| {
            b.iter_batched(
                || PatternReader::new(pattern.clone()),
                |source| {
                    let reader = zmq_framed_read(source);
                    let messages = block_on(drain_retained(reader));
                    assert_eq!(messages, pattern.messages);
                    black_box(messages);
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

// Repeat only message bytes after the greeting so the same reader survives
// every timed batch, including its adaptive state and any prefetched frames.
struct RepeatingReader(PatternReader);

impl AsyncRead for RepeatingReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.0.position == self.0.pattern.wire.len() {
            self.0.position = GREETING_STUB.len();
            self.0.next_boundary = 0;
        }
        Pin::new(&mut self.0).poll_read(cx, buffer)
    }
}

pub fn bench_read_buffer_steady(c: &mut Criterion) {
    let mut group = c.benchmark_group("framed_read/steady");
    bench_runtime::configure_group(&mut group);
    for (name, pattern) in [
        ("burst_small_burst", Pattern::small_messages(true)),
        ("gapped_batches", Pattern::gapped_batches()),
        (
            "large_boundaries",
            Pattern::sized_messages(338_729, 32, false, true),
        ),
        (
            "frame_65535_boundaries",
            Pattern::sized_messages(65_535, 64, false, true),
        ),
        (
            "frame_65536_boundaries",
            Pattern::sized_messages(65_536, 64, false, true),
        ),
        (
            "frame_65537_boundaries",
            Pattern::sized_messages(65_537, 64, false, true),
        ),
        (
            "frame_131071_boundaries",
            Pattern::sized_messages(131_071, 64, false, true),
        ),
        (
            "frame_131072_boundaries",
            Pattern::sized_messages(131_072, 64, false, true),
        ),
        (
            "frame_131073_boundaries",
            Pattern::sized_messages(131_073, 64, false, true),
        ),
    ] {
        group.throughput(Throughput::Bytes(
            (pattern.wire.len() - GREETING_STUB.len()) as u64,
        ));
        let mut reader = zmq_framed_read(RepeatingReader(PatternReader::new(pattern.clone())));
        assert!(matches!(
            block_on(reader.next()),
            Some(Ok(Message::Greeting(_)))
        ));
        let mut retained = VecDeque::with_capacity(65);
        // Prime a complete workload cycle before measuring the persistent reader.
        block_on(async {
            for _ in 0..pattern.messages {
                black_box(reader.next().await.unwrap().unwrap());
            }
        });
        group.bench_function(BenchmarkId::new(name, "default"), |b| {
            b.iter(|| {
                block_on(async {
                    for _ in 0..pattern.messages {
                        let Message::Message(message) = reader.next().await.unwrap().unwrap()
                        else {
                            panic!("expected message");
                        };
                        retained.push_back(message);
                        if retained.len() > 64 {
                            retained.pop_front();
                        }
                    }
                    black_box(&retained);
                });
            });
        });
    }
    group.finish();
}
