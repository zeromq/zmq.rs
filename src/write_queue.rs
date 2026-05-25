use crate::codec::{Message, ZmqFramedWrite};

use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};

const WRITE_BATCH_LIMIT: usize = 128;

/// Writes queued socket messages into the underlying framed sink in small batches.
pub(crate) async fn write_message_queue(
    mut queue_receiver: mpsc::Receiver<Message>,
    mut send_queue: ZmqFramedWrite,
) {
    while let Some(message) = queue_receiver.next().await {
        // When backlog already exists, feed a batch before flushing once to avoid per-message flush cost during bursts.
        if send_queue.feed(message).await.is_err() {
            break;
        }

        for _ in 1..WRITE_BATCH_LIMIT {
            let message = match queue_receiver.try_recv() {
                Ok(message) => message,
                Err(_) => break,
            };
            if send_queue.feed(message).await.is_err() {
                return;
            }
        }

        if send_queue.flush().await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::ZmqCodec;
    use crate::ZmqMessage;

    use bytes::Bytes;
    use futures::{AsyncWrite, SinkExt};
    use parking_lot::Mutex;
    use std::io;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};

    #[derive(Default)]
    struct WriteStats {
        bytes: Vec<u8>,
        flushes: usize,
    }

    struct RecordingWrite {
        stats: Arc<Mutex<WriteStats>>,
    }

    impl AsyncWrite for RecordingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let mut stats = self.stats.lock();
            stats.bytes.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            self.stats.lock().flushes += 1;
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[crate::async_rt::test]
    async fn test_write_message_queue_batches_ready_messages_into_one_flush() {
        let (mut sender, receiver) = mpsc::channel(8);
        sender
            .send(Message::Message(ZmqMessage::from(Bytes::from_static(b"a"))))
            .await
            .expect("send first message");
        sender
            .send(Message::Message(ZmqMessage::from(Bytes::from_static(b"b"))))
            .await
            .expect("send second message");
        sender
            .send(Message::Message(ZmqMessage::from(Bytes::from_static(b"c"))))
            .await
            .expect("send third message");
        drop(sender);

        let stats = Arc::new(Mutex::new(WriteStats::default()));
        let writer = RecordingWrite {
            stats: Arc::clone(&stats),
        };
        let send_queue = ZmqFramedWrite::new(Box::new(writer), ZmqCodec::new());

        write_message_queue(receiver, send_queue).await;

        let stats = stats.lock();
        assert_eq!(1, stats.flushes);
        assert_eq!(
            stats.bytes,
            vec![0x00, 0x01, b'a', 0x00, 0x01, b'b', 0x00, 0x01, b'c']
        );
    }
}
