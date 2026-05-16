use crate::codec::{CodecResult, Message, ZmqCodec};

use asynchronous_codec::{Decoder, FramedWrite};
use bytes::BytesMut;
use futures::{ready, AsyncRead, AsyncWrite, Stream};
use std::io::{self, ErrorKind};
use std::pin::Pin;
use std::task::{Context, Poll};

// Enables us to have multiple bounds on the dyn trait in `InnerFramed`
pub trait FrameableRead: AsyncRead + Unpin + Send + Sync {}
impl<T> FrameableRead for T where T: AsyncRead + Unpin + Send + Sync {}
pub trait FrameableWrite: AsyncWrite + Unpin + Send + Sync {}
impl<T> FrameableWrite for T where T: AsyncWrite + Unpin + Send + Sync {}

pub(crate) type ZmqFramedWrite = asynchronous_codec::FramedWrite<Box<dyn FrameableWrite>, ZmqCodec>;

/// A `Stream` of ZMTP messages decoded from an `AsyncRead`.
///
/// This mirrors `asynchronous_codec::FramedRead` but uses a larger reusable read
/// buffer. The PUSH/PULL OMQ comparison stresses 8 KiB payloads, where small
/// read chunks tend to split frame headers and payloads across polls.
pub struct ZmqFramedRead {
    inner: Box<dyn FrameableRead>,
    codec: ZmqCodec,
    buffer: BytesMut,
    read_buf: Box<[u8]>,
}

impl ZmqFramedRead {
    pub(crate) fn new(inner: Box<dyn FrameableRead>) -> Self {
        Self {
            inner,
            codec: ZmqCodec::new(),
            buffer: BytesMut::with_capacity(READ_BUFFER_CAPACITY),
            read_buf: vec![0; READ_BUFFER_CAPACITY].into_boxed_slice(),
        }
    }
}

impl Stream for ZmqFramedRead {
    type Item = CodecResult<Message>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;

        if let Some(item) = this.codec.decode(&mut this.buffer)? {
            return Poll::Ready(Some(Ok(item)));
        }

        loop {
            let read = {
                let inner = &mut this.inner;
                let read_buf = &mut this.read_buf;
                ready!(Pin::new(inner).poll_read(cx, read_buf))?
            };
            let ended = read == 0;
            this.buffer.extend_from_slice(&this.read_buf[..read]);

            match this.codec.decode(&mut this.buffer)? {
                Some(item) => return Poll::Ready(Some(Ok(item))),
                None if ended => {
                    if this.buffer.is_empty() {
                        return Poll::Ready(None);
                    }

                    match this.codec.decode_eof(&mut this.buffer)? {
                        Some(item) => return Poll::Ready(Some(Ok(item))),
                        None if this.buffer.is_empty() => return Poll::Ready(None),
                        None => {
                            return Poll::Ready(Some(Err(io::Error::new(
                                ErrorKind::UnexpectedEof,
                                "bytes remaining in stream",
                            )
                            .into())));
                        }
                    }
                }
                None => {}
            }
        }
    }
}

const READ_BUFFER_CAPACITY: usize = 64 * 1024;

/// Equivalent to [`asynchronous_codec::Framed<T, ZmqCodec>`]
pub struct FramedIo {
    pub read_half: ZmqFramedRead,
    pub write_half: ZmqFramedWrite,
}

impl FramedIo {
    pub fn new(read_half: Box<dyn FrameableRead>, write_half: Box<dyn FrameableWrite>) -> Self {
        let read_half = ZmqFramedRead::new(read_half);
        let write_half = FramedWrite::new(write_half, ZmqCodec::new());
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
    use crate::codec::{ZmqCodec, ZmqGreeting};
    use crate::ZmqMessage;
    use asynchronous_codec::Encoder;
    use bytes::Bytes;
    use futures::executor::block_on;
    use futures::StreamExt;
    use std::io::Error;
    use std::sync::{Arc, Mutex};

    #[test]
    fn framed_read_serves_buffered_messages_without_extra_reads() {
        let payload = Bytes::from(vec![0xCD; 8192]);
        let first = ZmqMessage::from(payload.clone());
        let second = ZmqMessage::from(payload.clone());
        let encoded = encode_greeting_and_messages(&[first, second]);
        let (reader, read_calls) = RecordingReader::new(encoded, READ_BUFFER_CAPACITY);

        let mut framed = ZmqFramedRead::new(Box::new(reader));
        block_on(async {
            expect_greeting(framed.next().await);
            assert_eq!(*read_calls.lock().expect("read calls lock"), 1);

            let message = expect_message(framed.next().await);
            expect_single_frame(message, payload.as_ref());
            assert_eq!(*read_calls.lock().expect("read calls lock"), 1);

            let message = expect_message(framed.next().await);
            expect_single_frame(message, payload.as_ref());
            assert_eq!(*read_calls.lock().expect("read calls lock"), 1);
        });
    }

    #[test]
    fn framed_read_preserves_fragmented_multipart_messages() {
        let message = ZmqMessage::try_from(vec![
            Bytes::from_static(b"first"),
            Bytes::from(vec![0xAB; 300]),
            Bytes::from_static(b"tail"),
        ])
        .expect("non-empty multipart");
        let encoded = encode_greeting_and_messages(&[message]);
        let (reader, _read_calls) = RecordingReader::new(encoded, 5);

        let mut framed = ZmqFramedRead::new(Box::new(reader));
        block_on(async {
            expect_greeting(framed.next().await);
            let message = expect_message(framed.next().await);
            let frames = message.into_vec();

            assert_eq!(frames.len(), 3);
            assert_eq!(frames[0].as_ref(), b"first");
            assert_eq!(frames[1].as_ref(), &[0xAB; 300]);
            assert_eq!(frames[2].as_ref(), b"tail");
        });
    }

    fn encode_greeting_and_messages(messages: &[ZmqMessage]) -> Vec<u8> {
        let mut codec = ZmqCodec::new();
        let mut dst = BytesMut::new();
        let greeting = Message::Greeting(ZmqGreeting::default());
        codec.encode(greeting, &mut dst).expect("greeting encode");

        for message in messages {
            let outbound = Message::Message(message.clone());
            codec.encode(outbound, &mut dst).expect("message encode");
        }

        dst.to_vec()
    }

    fn expect_greeting(item: Option<CodecResult<Message>>) {
        match item.expect("stream item").expect("decode succeeds") {
            Message::Greeting(_) => {}
            other => panic!("expected greeting, got {other:?}"),
        }
    }

    fn expect_message(item: Option<CodecResult<Message>>) -> ZmqMessage {
        match item.expect("stream item").expect("decode succeeds") {
            Message::Message(message) => message,
            other => panic!("expected message, got {other:?}"),
        }
    }

    fn expect_single_frame(message: ZmqMessage, expected: &[u8]) {
        let frames = message.into_vec();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].as_ref(), expected);
    }

    struct RecordingReader {
        data: Vec<u8>,
        offset: usize,
        max_chunk: usize,
        read_calls: Arc<Mutex<usize>>,
    }

    impl RecordingReader {
        fn new(data: Vec<u8>, max_chunk: usize) -> (Self, Arc<Mutex<usize>>) {
            let read_calls = Arc::new(Mutex::new(0));
            (
                Self {
                    data,
                    offset: 0,
                    max_chunk,
                    read_calls: read_calls.clone(),
                },
                read_calls,
            )
        }
    }

    impl AsyncRead for RecordingReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut [u8],
        ) -> Poll<Result<usize, Error>> {
            *self.read_calls.lock().expect("read calls lock") += 1;
            if self.offset == self.data.len() {
                return Poll::Ready(Ok(0));
            }

            let len = buf
                .len()
                .min(self.max_chunk)
                .min(self.data.len() - self.offset);
            buf[..len].copy_from_slice(&self.data[self.offset..self.offset + len]);
            self.offset += len;
            Poll::Ready(Ok(len))
        }
    }
}
