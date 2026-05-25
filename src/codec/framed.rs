use crate::codec::ZmqCodec;

use super::error::{CodecError, CodecResult};
use super::Message;

use asynchronous_codec::{Decoder, FramedWrite};
use bytes::BytesMut;
use futures::{ready, AsyncRead, AsyncWrite, Stream};

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

// Enables us to have multiple bounds on the dyn trait in `InnerFramed`
pub trait FrameableRead: AsyncRead + Unpin + Send + Sync {}
impl<T> FrameableRead for T where T: AsyncRead + Unpin + Send + Sync {}
pub trait FrameableWrite: AsyncWrite + Unpin + Send + Sync {}
impl<T> FrameableWrite for T where T: AsyncWrite + Unpin + Send + Sync {}

pub(crate) type ZmqFramedWrite = asynchronous_codec::FramedWrite<Box<dyn FrameableWrite>, ZmqCodec>;

const READ_CHUNK_SIZE: usize = 64 * 1024;

/// ZMTP framed reader with a larger read chunk.
///
/// `asynchronous-codec` defaults to 8 KiB reads from the underlying transport.
/// Raising the chunk to 64 KiB reduces read/poll loops on the framed read path
/// during large IPC/TCP payload bursts.
pub struct ZmqFramedRead {
    inner: Box<dyn FrameableRead>,
    codec: ZmqCodec,
    buffer: BytesMut,
    read_scratch: Box<[u8]>,
}

impl ZmqFramedRead {
    fn new(inner: Box<dyn FrameableRead>) -> Self {
        Self {
            inner,
            codec: ZmqCodec::new(),
            buffer: BytesMut::with_capacity(READ_CHUNK_SIZE),
            read_scratch: vec![0; READ_CHUNK_SIZE].into_boxed_slice(),
        }
    }
}

impl Stream for ZmqFramedRead {
    type Item = CodecResult<Message>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;

        // Consume buffered data first; read from the transport only when the current buffer cannot produce a full message.
        if let Some(item) = this.codec.decode(&mut this.buffer)? {
            return Poll::Ready(Some(Ok(item)));
        }

        loop {
            let n = ready!(Pin::new(&mut this.inner).poll_read(cx, &mut this.read_scratch))?;
            this.buffer.extend_from_slice(&this.read_scratch[..n]);

            let ended = n == 0;
            match this.codec.decode(&mut this.buffer)? {
                Some(item) => return Poll::Ready(Some(Ok(item))),
                None if ended => {
                    // On EOF, give the decoder one final chance to consume the remaining buffer before reporting truncated input.
                    if this.buffer.is_empty() {
                        return Poll::Ready(None);
                    }
                    match this.codec.decode_eof(&mut this.buffer)? {
                        Some(item) => return Poll::Ready(Some(Ok(item))),
                        None if this.buffer.is_empty() => return Poll::Ready(None),
                        None => {
                            return Poll::Ready(Some(Err(CodecError::Io(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "bytes remaining in stream",
                            )))));
                        }
                    }
                }
                None => {}
            }
        }
    }
}

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
    use crate::async_rt;
    use crate::codec::ZmqGreeting;

    use bytes::{Bytes, BytesMut};
    use futures::io::Cursor;
    use futures::StreamExt;
    use std::io::ErrorKind;

    fn encoded_stream(frames: &[&[u8]]) -> Vec<u8> {
        let mut bytes = BytesMut::from(ZmqGreeting::default());
        for frame in frames {
            bytes.extend_from_slice(&[0, frame.len() as u8]);
            bytes.extend_from_slice(frame);
        }
        bytes.to_vec()
    }

    #[async_rt::test]
    async fn test_zmq_framed_read_yields_buffered_messages_before_reading_more() {
        let input = encoded_stream(&[b"first", b"second"]);
        let mut reader = ZmqFramedRead::new(Box::new(Cursor::new(input)));

        assert!(matches!(
            reader.next().await,
            Some(Ok(Message::Greeting(_)))
        ));

        let first = reader
            .next()
            .await
            .expect("first message")
            .expect("decode first");
        let Message::Message(first) = first else {
            panic!("unexpected first frame type");
        };
        assert_eq!(first.get(0), Some(&Bytes::from_static(b"first")));

        let second = reader
            .next()
            .await
            .expect("second message")
            .expect("decode second");
        let Message::Message(second) = second else {
            panic!("unexpected second frame type");
        };
        assert_eq!(second.get(0), Some(&Bytes::from_static(b"second")));

        assert!(reader.next().await.is_none());
    }

    #[async_rt::test]
    async fn test_zmq_framed_read_reports_truncated_frame_at_eof() {
        let mut input = BytesMut::from(ZmqGreeting::default()).to_vec();
        input.extend_from_slice(&[0, 5, b'a']);
        let mut reader = ZmqFramedRead::new(Box::new(Cursor::new(input)));

        assert!(matches!(
            reader.next().await,
            Some(Ok(Message::Greeting(_)))
        ));

        let error = reader
            .next()
            .await
            .expect("truncated frame should produce an item")
            .expect_err("truncated frame should fail");
        match error {
            CodecError::Io(error) => assert_eq!(error.kind(), ErrorKind::UnexpectedEof),
            other => panic!("unexpected error type: {other:?}"),
        }
    }
}
