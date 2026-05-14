use crate::codec::{CodecError, Message, ZmqCodec};
use crate::ZmqMessage;

use asynchronous_codec::{Encoder, FramedRead};
use bytes::{Buf, Bytes, BytesMut};
use futures::{ready, Sink};
use futures::{AsyncRead, AsyncWrite};
use std::io::{Error, ErrorKind, IoSlice};
use std::pin::Pin;
use std::task::{Context, Poll};

// Enables us to have multiple bounds on the dyn trait in `InnerFramed`
pub trait FrameableRead: AsyncRead + Unpin + Send + Sync {}
impl<T> FrameableRead for T where T: AsyncRead + Unpin + Send + Sync {}
pub trait FrameableWrite: AsyncWrite + Unpin + Send + Sync {
    fn supports_write_vectored(&self) -> bool;
}

pub(crate) type ZmqFramedRead = asynchronous_codec::FramedRead<Box<dyn FrameableRead>, ZmqCodec>;

impl<T> FrameableWrite for futures::io::WriteHalf<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync,
{
    fn supports_write_vectored(&self) -> bool {
        // futures::AsyncWrite has no capability flag like Tokio's
        // is_write_vectored(), so keep generic split writes on the safe path.
        false
    }
}

impl FrameableWrite for futures::io::Sink {
    fn supports_write_vectored(&self) -> bool {
        false
    }
}

/// A `Sink` of ZMTP messages encoded to an `AsyncWrite`.
///
/// The ordinary codec encoder still exists for pure codec callers. This write
/// half bypasses its contiguous `BytesMut` payload copy for large or multipart
/// messages when the erased transport can perform vectored writes.
pub struct ZmqFramedWrite {
    inner: Box<dyn FrameableWrite>,
    codec: ZmqCodec,
    buffer: BytesMut,
    pending: Option<PendingWrite>,
    high_water_mark: usize,
}

impl ZmqFramedWrite {
    pub fn new(inner: Box<dyn FrameableWrite>) -> Self {
        Self {
            inner,
            codec: ZmqCodec::new(),
            buffer: BytesMut::with_capacity(1028 * 8),
            pending: None,
            high_water_mark: 131_072,
        }
    }

    fn should_write_vectored(&self, message: &ZmqMessage) -> bool {
        self.inner.supports_write_vectored()
            && self.buffer.is_empty()
            && self.pending.is_none()
            && (message.len() > 1
                || message
                    .iter()
                    .any(|frame| frame.len() >= VECTORED_SINGLE_FRAME_MIN_PAYLOAD))
    }

    fn poll_drain_pending(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), CodecError>> {
        while self.pending.is_some() {
            let num_write = {
                let pending = self.pending.as_ref().expect("pending write");
                let mut bufs = Vec::with_capacity(MAX_VECTORED_SLICES);
                pending.fill_io_slices(&mut bufs);
                ready!(Pin::new(&mut self.inner).poll_write_vectored(cx, &bufs))?
            };

            if num_write == 0 {
                return Poll::Ready(Err(err_eof().into()));
            }

            let pending = self.pending.as_mut().expect("pending write");
            pending.advance(num_write);
            if pending.is_complete() {
                self.pending = None;
            }
        }
        Poll::Ready(Ok(()))
    }

    fn poll_drain_buffer(
        &mut self,
        cx: &mut Context<'_>,
        drain_all: bool,
    ) -> Poll<Result<(), CodecError>> {
        while !self.buffer.is_empty() && (drain_all || self.buffer.len() >= self.high_water_mark) {
            let num_write = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.buffer))?;

            if num_write == 0 {
                return Poll::Ready(Err(err_eof().into()));
            }

            self.buffer.advance(num_write);
        }
        Poll::Ready(Ok(()))
    }
}

impl<'a> Sink<&'a Message> for ZmqFramedWrite {
    type Error = CodecError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.poll_drain_pending(cx))?;
        self.poll_drain_buffer(cx, false)
    }

    fn start_send(mut self: Pin<&mut Self>, item: &'a Message) -> Result<(), Self::Error> {
        match item {
            Message::Message(message) if self.should_write_vectored(message) => {
                self.pending = Some(PendingWrite::from_message(message));
                Ok(())
            }
            _ => {
                let this = &mut *self;
                this.codec.encode(item, &mut this.buffer)
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.poll_drain_pending(cx))?;
        ready!(self.poll_drain_buffer(cx, true))?;
        Pin::new(&mut self.inner).poll_flush(cx).map_err(Into::into)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(self.as_mut().poll_flush(cx))?;
        Pin::new(&mut self.inner).poll_close(cx).map_err(Into::into)
    }
}

const MAX_VECTORED_SLICES: usize = 64;
// Keep modest single-frame sends on the contiguous path; writev mainly pays off
// once the avoided payload copy is large enough. Multipart stays eligible.
const VECTORED_SINGLE_FRAME_MIN_PAYLOAD: usize = 16 * 1024;

struct PendingWrite {
    segments: Vec<PendingSegment>,
    index: usize,
    offset: usize,
}

impl PendingWrite {
    fn from_message(message: &ZmqMessage) -> Self {
        let mut frames = message.iter().peekable();
        let mut segments = Vec::new();

        while let Some(payload) = frames.next() {
            let more = frames.peek().is_some();
            segments.push(PendingSegment::Header(FrameHeader::new(payload, more)));
            if !payload.is_empty() {
                segments.push(PendingSegment::Payload(payload.clone()));
            }
        }

        Self {
            segments,
            index: 0,
            offset: 0,
        }
    }

    fn fill_io_slices<'a>(&'a self, bufs: &mut Vec<IoSlice<'a>>) {
        for segment in self.segments[self.index..].iter() {
            if bufs.len() == MAX_VECTORED_SLICES {
                break;
            }

            let mut bytes = segment.as_slice();
            if bufs.is_empty() && self.offset > 0 {
                bytes = &bytes[self.offset..];
            }
            if !bytes.is_empty() {
                bufs.push(IoSlice::new(bytes));
            }
        }
    }

    fn advance(&mut self, mut written: usize) {
        while written > 0 && !self.is_complete() {
            let remaining = self.segments[self.index].len() - self.offset;
            if written < remaining {
                self.offset += written;
                return;
            }

            written -= remaining;
            self.index += 1;
            self.offset = 0;
        }
    }

    fn is_complete(&self) -> bool {
        self.index >= self.segments.len()
    }
}

enum PendingSegment {
    Header(FrameHeader),
    Payload(Bytes),
}

impl PendingSegment {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Header(header) => header.as_slice(),
            Self::Payload(payload) => payload.as_ref(),
        }
    }

    fn len(&self) -> usize {
        self.as_slice().len()
    }
}

struct FrameHeader {
    bytes: [u8; 9],
    len: usize,
}

impl FrameHeader {
    fn new(payload: &Bytes, more: bool) -> Self {
        let mut bytes = [0; 9];
        if more {
            bytes[0] |= 0b0000_0001;
        }

        let payload_len = payload.len();
        let len = if payload_len > 255 {
            bytes[0] |= 0b0000_0010;
            bytes[1..].copy_from_slice(&(payload_len as u64).to_be_bytes());
            9
        } else {
            bytes[1] = payload_len as u8;
            2
        };

        Self { bytes, len }
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

fn err_eof() -> Error {
    Error::new(ErrorKind::UnexpectedEof, "End of file")
}

/// Equivalent to [`asynchronous_codec::Framed<T, ZmqCodec>`]
pub struct FramedIo {
    pub read_half: ZmqFramedRead,
    pub write_half: ZmqFramedWrite,
}

impl FramedIo {
    pub fn new(read_half: Box<dyn FrameableRead>, write_half: Box<dyn FrameableWrite>) -> Self {
        let read_half = FramedRead::new(read_half, ZmqCodec::new());
        let write_half = ZmqFramedWrite::new(write_half);
        Self {
            read_half,
            write_half,
        }
    }

    pub fn into_parts(self) -> (ZmqFramedRead, ZmqFramedWrite) {
        (self.read_half, self.write_half)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use futures::SinkExt;
    use std::sync::{Arc, Mutex};

    #[test]
    fn vectored_write_handles_partial_large_multipart_message() {
        let (writer, stats) = RecordingWriter::new(true, 3);
        let message = ZmqMessage::try_from(vec![
            Bytes::from_static(b"small"),
            Bytes::from(vec![0xAB; 300]),
            Bytes::from_static(b"tail"),
        ])
        .expect("non-empty multipart");
        let expected = encode_with_codec(message.clone());

        let mut framed = ZmqFramedWrite::new(Box::new(writer));
        let outbound = Message::Message(message);
        block_on(framed.send(&outbound)).expect("send succeeds");

        let stats = stats.lock().expect("stats lock");
        assert_eq!(stats.bytes, expected);
        assert!(stats.vectored_calls > 1);
        assert_eq!(stats.write_calls, 0);
    }

    #[test]
    fn contiguous_write_is_used_when_transport_does_not_support_vectored_writes() {
        let (writer, stats) = RecordingWriter::new(false, 64);
        let message = ZmqMessage::try_from(vec![
            Bytes::from(vec![0xCD; 300]),
            Bytes::from_static(b"second"),
        ])
        .expect("non-empty multipart");
        let expected = encode_with_codec(message.clone());

        let mut framed = ZmqFramedWrite::new(Box::new(writer));
        let outbound = Message::Message(message);
        block_on(framed.send(&outbound)).expect("send succeeds");

        let stats = stats.lock().expect("stats lock");
        assert_eq!(stats.bytes, expected);
        assert_eq!(stats.vectored_calls, 0);
        assert!(stats.write_calls > 1);
    }

    fn encode_with_codec(message: ZmqMessage) -> Vec<u8> {
        let mut codec = ZmqCodec::new();
        let mut dst = BytesMut::new();
        let outbound = Message::Message(message);
        codec.encode(&outbound, &mut dst).expect("codec encode");
        dst.to_vec()
    }

    #[derive(Default)]
    struct WriteStats {
        bytes: Vec<u8>,
        max_chunk: usize,
        vectored_calls: usize,
        write_calls: usize,
    }

    struct RecordingWriter {
        stats: Arc<Mutex<WriteStats>>,
        supports_vectored: bool,
    }

    impl RecordingWriter {
        fn new(supports_vectored: bool, max_chunk: usize) -> (Self, Arc<Mutex<WriteStats>>) {
            let stats = Arc::new(Mutex::new(WriteStats {
                max_chunk,
                ..WriteStats::default()
            }));
            (
                Self {
                    stats: stats.clone(),
                    supports_vectored,
                },
                stats,
            )
        }
    }

    impl AsyncWrite for RecordingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<Result<usize, Error>> {
            let mut stats = self.stats.lock().expect("stats lock");
            stats.write_calls += 1;
            let len = buf.len().min(stats.max_chunk);
            stats.bytes.extend_from_slice(&buf[..len]);
            Poll::Ready(Ok(len))
        }

        fn poll_write_vectored(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            bufs: &[IoSlice<'_>],
        ) -> Poll<Result<usize, Error>> {
            let mut stats = self.stats.lock().expect("stats lock");
            stats.vectored_calls += 1;

            let mut remaining = stats.max_chunk;
            let mut written = 0;
            for buf in bufs {
                if remaining == 0 {
                    break;
                }
                let len = buf.len().min(remaining);
                stats.bytes.extend_from_slice(&buf[..len]);
                remaining -= len;
                written += len;
            }

            Poll::Ready(Ok(written))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
            Poll::Ready(Ok(()))
        }
    }

    impl FrameableWrite for RecordingWriter {
        fn supports_write_vectored(&self) -> bool {
            self.supports_vectored
        }
    }
}
