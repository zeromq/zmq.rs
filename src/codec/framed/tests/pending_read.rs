use super::*;
use std::collections::VecDeque;
use std::future::Future;

enum ReadStep {
    Data(Vec<u8>),
    Pending,
    Error,
}

struct ScriptedReader(VecDeque<ReadStep>);

impl AsyncRead for ScriptedReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match self.0.pop_front() {
            Some(ReadStep::Data(data)) => {
                assert!(data.len() <= buffer.len());
                buffer[..data.len()].copy_from_slice(&data);
                Poll::Ready(Ok(data.len()))
            }
            Some(ReadStep::Pending) => {
                // A reader may alter initialized storage without reporting bytes.
                buffer.fill(0xFF);
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Some(ReadStep::Error) => {
                buffer.fill(0xFF);
                Poll::Ready(Err(io::Error::from(ErrorKind::ConnectionReset)))
            }
            None => Poll::Ready(Ok(0)),
        }
    }
}

struct ReuseReader(usize);

impl AsyncRead for ReuseReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.0 > 0 {
            assert!(buffer.iter().all(|&byte| byte == 0xA5));
        }
        self.0 += 1;
        if self.0 < 3 {
            buffer.fill(0xA5);
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            buffer[..3].copy_from_slice(b"end");
            Poll::Ready(Ok(3))
        }
    }
}

#[test]
fn pending_read_reuses_initialized_space() {
    let mut reader =
        ZmqFramedRead::new(Box::new(ReuseReader(0)), Arc::new(SocketOptions::default()));
    reader.buffer.extend_from_slice(b"prefix");
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(reader.poll_read_into_buffer(&mut cx, 1).is_pending());
    assert!(reader.poll_read_into_buffer(&mut cx, 1).is_pending());
    assert!(matches!(
        reader.poll_read_into_buffer(&mut cx, 1),
        Poll::Ready(Ok(3))
    ));
    assert_eq!(&reader.buffer[..], b"prefixend");
}

#[async_rt::test]
async fn pending_read_never_decodes_unreported_bytes() {
    let source = ScriptedReader(VecDeque::from([
        ReadStep::Pending,
        ReadStep::Pending,
        ReadStep::Data(encoded_stream(&[b"message"])),
        ReadStep::Pending,
    ]));
    let reader = ZmqFramedRead::new(Box::new(source), Arc::new(SocketOptions::default()));

    let items: Vec<_> = reader.collect().await;

    assert_eq!(items.len(), 2);
    assert!(matches!(items[0], Ok(Message::Greeting(_))));
    let Ok(Message::Message(message)) = &items[1] else {
        panic!("expected one complete message");
    };
    assert_eq!(message.get(0).unwrap().as_ref(), b"message");
}

#[async_rt::test]
async fn pending_read_eof_preserves_truncated_frame_error() {
    let mut partial = encoded_stream(&[b"abc"]);
    partial.truncate(partial.len() - 2);
    let source = ScriptedReader(VecDeque::from([ReadStep::Data(partial), ReadStep::Pending]));
    let mut reader = ZmqFramedRead::new(Box::new(source), Arc::new(SocketOptions::default()));
    assert!(matches!(
        reader.next().await,
        Some(Ok(Message::Greeting(_)))
    ));

    let item = reader.next().await;

    assert!(
        matches!(item, Some(Err(CodecError::Io(error))) if error.kind() == ErrorKind::UnexpectedEof)
    );
}

#[async_rt::test]
async fn pending_read_error_preserves_partial_frame_for_retry() {
    let mut partial = encoded_stream(&[b"abc"]);
    partial.truncate(partial.len() - 2);
    let source = ScriptedReader(VecDeque::from([
        ReadStep::Data(partial),
        ReadStep::Pending,
        ReadStep::Error,
        ReadStep::Data(b"bc".to_vec()),
    ]));
    let mut reader = ZmqFramedRead::new(Box::new(source), Arc::new(SocketOptions::default()));
    assert!(matches!(
        reader.next().await,
        Some(Ok(Message::Greeting(_)))
    ));

    let error = reader.next().await;
    let item = reader.next().await;

    assert!(
        matches!(error, Some(Err(CodecError::Io(error))) if error.kind() == ErrorKind::ConnectionReset)
    );
    let Some(Ok(Message::Message(message))) = item else {
        panic!("expected the completed frame after retry");
    };
    assert_eq!(message.get(0).unwrap().as_ref(), b"abc");
    assert!(reader.next().await.is_none());
}

#[async_rt::test]
async fn cancelled_next_resumes_pending_read() {
    let source = ScriptedReader(VecDeque::from([
        ReadStep::Pending,
        ReadStep::Data(encoded_stream(&[b"message"])),
    ]));
    let mut reader = ZmqFramedRead::new(Box::new(source), Arc::new(SocketOptions::default()));
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    {
        let mut next = reader.next();
        assert!(Pin::new(&mut next).poll(&mut cx).is_pending());
    }

    let items: Vec<_> = reader.collect().await;

    assert_eq!(items.len(), 2);
    assert!(matches!(items[0], Ok(Message::Greeting(_))));
    let Ok(Message::Message(message)) = &items[1] else {
        panic!("expected one message after cancellation");
    };
    assert_eq!(message.get(0).unwrap().as_ref(), b"message");
}
