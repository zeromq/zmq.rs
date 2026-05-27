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

const INITIAL_READ_CHUNK_SIZE: usize = 128;
const MAX_READ_CHUNK_SIZE: usize = 64 * 1024;

/// ZMTP framed reader with an adaptive read chunk.
///
/// Starts with a small read chunk and grows only when the transport keeps
/// filling the requested space. This avoids a large default allocation while
/// still reducing read/poll loops for large receive bursts.
pub struct ZmqFramedRead {
    inner: Box<dyn FrameableRead>,
    codec: ZmqCodec,
    buffer: BytesMut,
    read_chunk_size: usize,
}

impl ZmqFramedRead {
    pub(crate) fn new(inner: Box<dyn FrameableRead>) -> Self {
        Self {
            inner,
            codec: ZmqCodec::new(),
            buffer: BytesMut::with_capacity(INITIAL_READ_CHUNK_SIZE),
            read_chunk_size: INITIAL_READ_CHUNK_SIZE,
        }
    }

    fn poll_read_into_buffer(
        &mut self,
        cx: &mut Context<'_>,
        min_read_size: usize,
    ) -> Poll<io::Result<usize>> {
        let start = self.buffer.len();
        let read_size = self
            .read_chunk_size
            .max(min_read_size)
            .min(MAX_READ_CHUNK_SIZE);

        // futures::AsyncRead requires an initialized &mut [u8]. Grow BytesMut
        // first, then trim back to the actual read length so the read path does
        // not need a scratch buffer and a second data copy.
        self.buffer.resize(start + read_size, 0);

        match Pin::new(&mut self.inner).poll_read(cx, &mut self.buffer[start..]) {
            Poll::Ready(Ok(bytes_read)) => {
                self.buffer.truncate(start + bytes_read);
                if bytes_read >= read_size && self.read_chunk_size < MAX_READ_CHUNK_SIZE {
                    self.read_chunk_size = (self.read_chunk_size * 2).min(MAX_READ_CHUNK_SIZE);
                }
                Poll::Ready(Ok(bytes_read))
            }
            Poll::Ready(Err(error)) => {
                self.buffer.truncate(start);
                Poll::Ready(Err(error))
            }
            Poll::Pending => {
                self.buffer.truncate(start);
                Poll::Pending
            }
        }
    }
}

impl Stream for ZmqFramedRead {
    type Item = CodecResult<Message>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;

        // Consume buffered data first; read from the socket only when the
        // current buffer cannot produce a full message.
        if let Some(item) = this.codec.decode(&mut this.buffer)? {
            return Poll::Ready(Some(Ok(item)));
        }

        loop {
            let min_read_size = this.codec.bytes_needed(this.buffer.len());
            let n = ready!(this.poll_read_into_buffer(cx, min_read_size))?;
            let ended = n == 0;
            match this.codec.decode(&mut this.buffer)? {
                Some(item) => return Poll::Ready(Some(Ok(item))),
                None if ended => {
                    // Give the decoder one final chance at EOF; leftover bytes
                    // mean the stream ended with a truncated frame.
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
    use futures::StreamExt;
    use futures::{io::Cursor, AsyncRead};
    use std::io::ErrorKind;
    use std::sync::{Arc, Mutex};

    struct RecordingReader {
        input: Vec<u8>,
        position: usize,
        requested_sizes: Arc<Mutex<Vec<usize>>>,
    }

    impl RecordingReader {
        fn new(input: Vec<u8>, requested_sizes: Arc<Mutex<Vec<usize>>>) -> Self {
            Self {
                input,
                position: 0,
                requested_sizes,
            }
        }
    }

    impl AsyncRead for RecordingReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buffer: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            self.requested_sizes.lock().unwrap().push(buffer.len());

            let remaining = self.input.len() - self.position;
            if remaining == 0 {
                return Poll::Ready(Ok(0));
            }

            let bytes_read = remaining.min(buffer.len());
            let end = self.position + bytes_read;
            buffer[..bytes_read].copy_from_slice(&self.input[self.position..end]);
            self.position = end;

            Poll::Ready(Ok(bytes_read))
        }
    }

    fn encoded_stream(frames: &[&[u8]]) -> Vec<u8> {
        let mut bytes = BytesMut::from(ZmqGreeting::default());
        for frame in frames {
            if frame.len() > u8::MAX as usize {
                bytes.extend_from_slice(&[0b0000_0010]);
                bytes.extend_from_slice(&(frame.len() as u64).to_be_bytes());
            } else {
                bytes.extend_from_slice(&[0, frame.len() as u8]);
            }
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

    #[async_rt::test]
    async fn test_zmq_framed_read_uses_decoder_demand_after_full_reads() {
        let frame = vec![b'a'; 600];
        let requested_sizes = Arc::new(Mutex::new(Vec::new()));
        let input = encoded_stream(&[&frame]);
        let input_len = input.len();
        let mut reader = ZmqFramedRead::new(Box::new(RecordingReader::new(
            input,
            Arc::clone(&requested_sizes),
        )));

        assert!(matches!(
            reader.next().await,
            Some(Ok(Message::Greeting(_)))
        ));

        let message = reader
            .next()
            .await
            .expect("message")
            .expect("decode message");
        let Message::Message(message) = message else {
            panic!("unexpected message type");
        };
        assert_eq!(message.get(0), Some(&Bytes::copy_from_slice(&frame)));

        let greeting_len = BytesMut::from(ZmqGreeting::default()).len();
        let encoded_frame_len = input_len - greeting_len;
        let initial_prefetched_frame_bytes = INITIAL_READ_CHUNK_SIZE - greeting_len;
        let remaining_frame_bytes = encoded_frame_len - initial_prefetched_frame_bytes;
        assert_eq!(
            *requested_sizes.lock().unwrap(),
            vec![INITIAL_READ_CHUNK_SIZE, remaining_frame_bytes]
        );
    }
}
