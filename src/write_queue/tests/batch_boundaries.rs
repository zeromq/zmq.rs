use super::*;

struct PartialPendingWrite {
    bytes: Arc<Mutex<Vec<u8>>>,
    pending: bool,
}

impl AsyncWrite for PartialPendingWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.pending {
            self.pending = false;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let written = buffer.len().min(7);
        self.bytes.lock().extend_from_slice(&buffer[..written]);
        Poll::Ready(Ok(written))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[crate::async_rt::test]
async fn multiple_batches_preserve_wire_order_across_pending_and_short_writes() {
    let (mut sender, receiver) = mpsc::channel(300);
    let mut expected = Vec::new();
    for sequence in 0..300u16 {
        let payload = sequence.to_le_bytes();
        sender
            .try_send(Message::Message(ZmqMessage::from(Bytes::copy_from_slice(
                &payload,
            ))))
            .unwrap();
        expected.extend_from_slice(&[0, 2]);
        expected.extend_from_slice(&payload);
    }
    drop(sender);
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let writer = PartialPendingWrite {
        bytes: Arc::clone(&bytes),
        pending: true,
    };
    let sink = ZmqFramedWrite::new(Box::new(writer), ZmqCodec::new());

    write_message_queue(receiver, sink).await.unwrap();

    assert_eq!(*bytes.lock(), expected);
}
