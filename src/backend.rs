use crate::async_rt;
use crate::codec::{FramedIo, Message, ZmqFramedRead, ZmqFramedWrite};
use crate::fair_queue::QueueInner;
use crate::util::PeerIdentity;
use crate::{
    MultiPeerBackend, SocketBackend, SocketEvent, SocketOptions, SocketType, ZmqError, ZmqResult,
};

use async_trait::async_trait;
use crossbeam_queue::SegQueue;
use futures::channel::mpsc;
use futures::future::poll_fn;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;

use std::collections::HashMap;
use std::sync::Arc;
use std::task::Poll;
use std::task::Waker;

/// Sender for notifying reconnection tasks when a peer disconnects.
pub(crate) type DisconnectNotifier = mpsc::Sender<PeerIdentity>;

const PUSH_SEND_QUEUE_CAPACITY: usize = 8 * 1024;
const PUSH_SEND_QUEUE_MAX_BYTES: usize = 2 * 1024 * 1024;
const PUSH_SEND_BATCH_MESSAGES: usize = 128;
const PUSH_SEND_BATCH_BYTES: usize = 512 * 1024;

pub(crate) struct Peer {
    send_queue: PeerSendQueue,
}

struct RoundRobinPeerGuard<'a> {
    queue: &'a SegQueue<PeerIdentity>,
    peer_id: Option<PeerIdentity>,
    requeue_on_drop: bool,
}

impl<'a> RoundRobinPeerGuard<'a> {
    fn new(queue: &'a SegQueue<PeerIdentity>, peer_id: PeerIdentity) -> Self {
        Self {
            queue,
            peer_id: Some(peer_id),
            requeue_on_drop: true,
        }
    }

    fn peer_id(&self) -> &PeerIdentity {
        self.peer_id.as_ref().expect("guard owns peer identity")
    }

    fn requeue(mut self) -> PeerIdentity {
        let peer_id = self.peer_id.take().expect("guard owns peer identity");
        self.requeue_on_drop = false;
        self.queue.push(peer_id.clone());
        peer_id
    }

    fn discard(mut self) -> PeerIdentity {
        self.requeue_on_drop = false;
        self.peer_id.take().expect("guard owns peer identity")
    }
}

impl Drop for RoundRobinPeerGuard<'_> {
    fn drop(&mut self) {
        if self.requeue_on_drop {
            if let Some(peer_id) = self.peer_id.take() {
                self.queue.push(peer_id);
            }
        }
    }
}

enum PeerSendQueue {
    Direct(ZmqFramedWrite),
    BatchedPush(PushBatchSender),
}

impl Peer {
    pub(crate) async fn send(&mut self, message: Message) -> ZmqResult<()> {
        match &mut self.send_queue {
            PeerSendQueue::Direct(send_queue) => send_queue.send(message).await.map_err(Into::into),
            PeerSendQueue::BatchedPush(sender) => sender.send(message).await,
        }
    }
}

struct PushBatchSender {
    sender: mpsc::Sender<QueuedPushMessage>,
    credits: Arc<PushQueueCredits>,
}

impl PushBatchSender {
    async fn send(&mut self, message: Message) -> ZmqResult<()> {
        let bytes = message_payload_bytes(&message);
        self.credits.reserve(bytes).await?;
        if self
            .sender
            .try_send(QueuedPushMessage { message, bytes })
            .is_ok()
        {
            Ok(())
        } else {
            self.credits.release(1, bytes);
            Err(ZmqError::BufferFull(
                "Failed to send message. Send queue full/broken",
            ))
        }
    }
}

struct QueuedPushMessage {
    message: Message,
    bytes: usize,
}

struct PushQueueCredits {
    state: Mutex<PushQueueCreditState>,
}

#[derive(Default)]
struct PushQueueCreditState {
    messages: usize,
    bytes: usize,
    closed: bool,
    waiters: Vec<Waker>,
}

impl PushQueueCredits {
    fn new() -> Self {
        Self {
            state: Mutex::new(PushQueueCreditState::default()),
        }
    }

    async fn reserve(&self, bytes: usize) -> ZmqResult<()> {
        poll_fn(|cx| {
            let mut state = self.state.lock();
            if state.closed {
                return Poll::Ready(Err(ZmqError::BufferFull(
                    "Failed to send message. Send queue full/broken",
                )));
            }

            if queue_has_capacity(&state, bytes) {
                state.messages += 1;
                state.bytes = state.bytes.saturating_add(bytes);
                return Poll::Ready(Ok(()));
            }

            if !state
                .waiters
                .iter()
                .any(|waker| waker.will_wake(cx.waker()))
            {
                state.waiters.push(cx.waker().clone());
            }
            Poll::Pending
        })
        .await
    }

    fn release(&self, messages: usize, bytes: usize) {
        let waiters = {
            let mut state = self.state.lock();
            state.messages = state.messages.saturating_sub(messages);
            state.bytes = state.bytes.saturating_sub(bytes);
            std::mem::take(&mut state.waiters)
        };
        wake_waiters(waiters);
    }

    fn close(&self) {
        let waiters = {
            let mut state = self.state.lock();
            state.closed = true;
            std::mem::take(&mut state.waiters)
        };
        wake_waiters(waiters);
    }
}

fn queue_has_capacity(state: &PushQueueCreditState, bytes: usize) -> bool {
    if state.messages >= PUSH_SEND_QUEUE_CAPACITY {
        return false;
    }

    let projected = state.bytes.saturating_add(bytes);
    projected <= PUSH_SEND_QUEUE_MAX_BYTES || state.messages == 0
}

fn wake_waiters(waiters: Vec<Waker>) {
    for waiter in waiters {
        waiter.wake();
    }
}

pub(crate) struct GenericSocketBackend {
    pub(crate) peers: scc::HashMap<PeerIdentity, Peer>,
    fair_queue_inner: Option<Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>>,
    pub(crate) round_robin: SegQueue<PeerIdentity>,
    socket_type: SocketType,
    socket_options: SocketOptions,
    pub(crate) socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
    /// Notifiers for reconnection tasks - keyed by `peer_id`
    disconnect_notifiers: Mutex<HashMap<PeerIdentity, DisconnectNotifier>>,
}

impl GenericSocketBackend {
    pub(crate) fn with_options(
        fair_queue_inner: Option<Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>>,
        socket_type: SocketType,
        options: SocketOptions,
    ) -> Self {
        Self {
            peers: scc::HashMap::new(),
            fair_queue_inner,
            round_robin: SegQueue::new(),
            socket_type,
            socket_options: options,
            socket_monitor: Mutex::new(None),
            disconnect_notifiers: Mutex::new(HashMap::new()),
        }
    }

    /// Register a notifier to be called when a peer disconnects.
    ///
    /// Used by reconnection tasks to be notified when they should attempt reconnection.
    #[allow(dead_code)] // Will be used when reconnection is added to more socket types
    pub(crate) fn register_disconnect_notifier(
        &self,
        peer_id: PeerIdentity,
        notifier: DisconnectNotifier,
    ) {
        self.disconnect_notifiers.lock().insert(peer_id, notifier);
    }

    /// Unregister a disconnect notifier for a peer.
    #[allow(dead_code)] // Will be used when reconnection is added to more socket types
    pub(crate) fn unregister_disconnect_notifier(&self, peer_id: &PeerIdentity) {
        self.disconnect_notifiers.lock().remove(peer_id);
    }

    pub(crate) async fn send_round_robin(&self, message: Message) -> ZmqResult<PeerIdentity> {
        // In normal scenario this will always be only 1 iteration
        // There can be special case when peer has disconnected and his id is still in
        // RR queue This happens because SegQueue don't have an api to delete
        // items from queue. So in such case we'll just pop item and skip it if
        // we don't have a matching peer in peers map
        loop {
            let next_peer_id = match self.round_robin.pop() {
                Some(peer) => peer,
                None => match message {
                    Message::Greeting(_) => {
                        return Err(ZmqError::Socket("Sending greeting is not supported"))
                    }
                    Message::Command(_) => {
                        return Err(ZmqError::Socket("Sending commands is not supported"))
                    }
                    Message::Message(m) => {
                        return Err(ZmqError::ReturnToSender {
                            reason: "Not connected to peers. Unable to send messages",
                            message: m,
                        })
                    }
                },
            };
            let peer_guard = RoundRobinPeerGuard::new(&self.round_robin, next_peer_id);
            let send_result =
                if let Some(mut peer) = self.peers.get_async(peer_guard.peer_id()).await {
                    peer.send(message).await
                } else {
                    peer_guard.discard();
                    continue;
                };
            return match send_result {
                Ok(()) => {
                    let peer_id = peer_guard.requeue();
                    Ok(peer_id)
                }
                Err(e) => {
                    let peer_id = peer_guard.discard();
                    self.peer_disconnected(&peer_id);
                    Err(e)
                }
            };
        }
    }
}

impl SocketBackend for GenericSocketBackend {
    fn socket_type(&self) -> SocketType {
        self.socket_type
    }

    fn socket_options(&self) -> &SocketOptions {
        &self.socket_options
    }

    fn shutdown(&self) {
        self.peers.clear_sync();
        // Clear fair_queue streams to ensure TCP connections are closed
        // even when reconnect tasks still hold Arc references to the backend
        if let Some(inner) = &self.fair_queue_inner {
            inner.lock().clear();
        }
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

#[async_trait]
impl MultiPeerBackend for GenericSocketBackend {
    async fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        let (recv_queue, send_queue) = io.into_parts();
        let send_queue = self.peer_send_queue(peer_id.clone(), send_queue);
        self.peers
            .upsert_async(peer_id.clone(), Peer { send_queue })
            .await;
        self.round_robin.push(peer_id.clone());
        match &self.fair_queue_inner {
            None => {}
            Some(inner) => {
                inner.lock().insert(peer_id.clone(), recv_queue);
            }
        };
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        let was_connected = self.peers.remove_sync(peer_id).is_some();
        match &self.fair_queue_inner {
            None => {}
            Some(inner) => {
                inner.lock().remove(peer_id);
            }
        };

        if was_connected {
            if let Some(monitor) = self.socket_monitor.lock().as_mut() {
                let _ = monitor.try_send(SocketEvent::Disconnected(peer_id.clone()));
            }
        }

        // Notify reconnection task if registered
        if let Some(mut notifier) = self.disconnect_notifiers.lock().remove(peer_id) {
            // Use try_send to avoid blocking - if channel is full, the reconnect task
            // will eventually notice the peer is gone
            let _ = notifier.try_send(peer_id.clone());
        }
    }
}

impl GenericSocketBackend {
    fn peer_send_queue(
        self: &Arc<Self>,
        peer_id: PeerIdentity,
        mut send_queue: ZmqFramedWrite,
    ) -> PeerSendQueue {
        if self.socket_type != SocketType::PUSH {
            return PeerSendQueue::Direct(send_queue);
        }

        send_queue.set_send_high_water_mark(PUSH_SEND_BATCH_BYTES);
        let credits = Arc::new(PushQueueCredits::new());
        let (sender, receiver) = mpsc::channel(PUSH_SEND_QUEUE_CAPACITY);
        async_rt::task::spawn(run_push_send_queue(
            self.clone(),
            peer_id,
            send_queue,
            receiver,
            credits.clone(),
        ));
        PeerSendQueue::BatchedPush(PushBatchSender { sender, credits })
    }
}

async fn run_push_send_queue(
    backend: Arc<GenericSocketBackend>,
    peer_id: PeerIdentity,
    mut send_queue: ZmqFramedWrite,
    mut receiver: mpsc::Receiver<QueuedPushMessage>,
    credits: Arc<PushQueueCredits>,
) {
    while let Some(first) = receiver.next().await {
        let mut batch_bytes = first.bytes;
        let mut batch_messages = 1;

        if send_queue.feed(first.message).await.is_err() {
            credits.close();
            backend.peer_disconnected(&peer_id);
            return;
        }

        while batch_messages < PUSH_SEND_BATCH_MESSAGES && batch_bytes < PUSH_SEND_BATCH_BYTES {
            let next = match receiver.try_recv() {
                Ok(message) => message,
                Err(_) => break,
            };

            batch_bytes += next.bytes;
            batch_messages += 1;
            if send_queue.feed(next.message).await.is_err() {
                credits.close();
                backend.peer_disconnected(&peer_id);
                return;
            }
        }

        credits.release(batch_messages, batch_bytes);

        if send_queue.flush().await.is_err() {
            credits.close();
            backend.peer_disconnected(&peer_id);
            return;
        }
    }

    let _ = send_queue.flush().await;
    credits.close();
}

fn message_payload_bytes(message: &Message) -> usize {
    match message {
        Message::Message(message) => message.frame_iter().map(bytes::Bytes::len).sum(),
        Message::Greeting(_) | Message::Command(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_rt;
    use crate::codec::FramedIo;
    use crate::util::PeerIdentity;
    use crate::{SocketEvent, SocketOptions, SocketType, ZmqMessage};

    use futures::channel::mpsc;
    use futures::{AsyncWrite, StreamExt};

    use std::io::{self, ErrorKind};
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use std::time::Duration;

    #[async_rt::test]
    async fn batched_push_writer_failure_removes_peer_and_reports_disconnect() {
        let backend = push_backend();
        let peer_id = PeerIdentity::try_from(b"peer-a".as_slice()).unwrap();
        let (monitor_sender, mut monitor_receiver) = mpsc::channel(8);
        backend.socket_monitor.lock().replace(monitor_sender);

        backend
            .clone()
            .peer_connected(&peer_id, framed_io(FailingWrite))
            .await;

        backend
            .send_round_robin(Message::Message(ZmqMessage::from("first")))
            .await
            .unwrap();

        let event = async_rt::task::timeout(Duration::from_secs(2), monitor_receiver.next())
            .await
            .expect("timed out waiting for disconnect event")
            .expect("monitor closed");
        assert!(matches!(event, SocketEvent::Disconnected(id) if id == peer_id));

        let err = backend
            .send_round_robin(Message::Message(ZmqMessage::from("second")))
            .await
            .unwrap_err();
        assert!(matches!(err, ZmqError::ReturnToSender { .. }));
    }

    #[async_rt::test]
    async fn cancelled_backpressure_wait_requeues_round_robin_peer() {
        let backend = push_backend();
        let peer_id = PeerIdentity::try_from(b"peer-b".as_slice()).unwrap();

        backend
            .clone()
            .peer_connected(&peer_id, framed_io(PendingWrite))
            .await;

        let mut blocked_at = None;
        for seq in 0..=(PUSH_SEND_QUEUE_CAPACITY + PUSH_SEND_BATCH_MESSAGES) {
            match async_rt::task::timeout(
                Duration::from_secs(2),
                backend.send_round_robin(Message::Message(ZmqMessage::from(format!("fill-{seq}")))),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => panic!("send failed before queue filled: {err}"),
                Err(_) => {
                    blocked_at = Some(seq);
                    break;
                }
            }
        }

        let blocked_at = blocked_at.expect("send never waited on the full PUSH queue");
        assert!(blocked_at >= PUSH_SEND_QUEUE_CAPACITY);
        assert_eq!(backend.round_robin.pop(), Some(peer_id));
    }

    #[async_rt::test]
    async fn large_push_messages_wait_on_byte_hwm() {
        let backend = push_backend();
        let peer_id = PeerIdentity::try_from(b"peer-c".as_slice()).unwrap();

        backend
            .clone()
            .peer_connected(&peer_id, framed_io(PendingWrite))
            .await;

        let payload = vec![0xAB; 8192];
        let mut blocked_at = None;
        for seq in 0..PUSH_SEND_QUEUE_CAPACITY {
            match async_rt::task::timeout(
                Duration::from_secs(2),
                backend.send_round_robin(Message::Message(ZmqMessage::from(payload.clone()))),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => panic!("send failed before byte queue filled: {err}"),
                Err(_) => {
                    blocked_at = Some(seq);
                    break;
                }
            }
        }

        let blocked_at = blocked_at.expect("send never waited on the byte-limited PUSH queue");
        assert!(
            blocked_at < PUSH_SEND_QUEUE_CAPACITY,
            "byte HWM should apply before message HWM, blocked at {blocked_at}"
        );
        assert_eq!(backend.round_robin.pop(), Some(peer_id));
    }

    #[async_rt::test]
    async fn flush_pending_push_messages_remain_byte_limited() {
        let backend = push_backend();
        let peer_id = PeerIdentity::try_from(b"peer-d".as_slice()).unwrap();

        backend
            .clone()
            .peer_connected(&peer_id, framed_io(FlushPendingWrite))
            .await;

        let payload = vec![0xCD; 8192];
        let mut blocked_at = None;
        for seq in 0..PUSH_SEND_QUEUE_CAPACITY {
            match async_rt::task::timeout(
                Duration::from_secs(2),
                backend.send_round_robin(Message::Message(ZmqMessage::from(payload.clone()))),
            )
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => panic!("send failed before flush-pending queue filled: {err}"),
                Err(_) => {
                    blocked_at = Some(seq);
                    break;
                }
            }
        }

        let blocked_at = blocked_at.expect("send never waited while writer flush was pending");
        assert!(
            blocked_at < PUSH_SEND_QUEUE_CAPACITY,
            "byte HWM should still apply while writer flush is pending, blocked at {blocked_at}"
        );
        assert_eq!(backend.round_robin.pop(), Some(peer_id));
    }

    fn push_backend() -> Arc<GenericSocketBackend> {
        Arc::new(GenericSocketBackend::with_options(
            None,
            SocketType::PUSH,
            SocketOptions::default(),
        ))
    }

    fn framed_io<W>(writer: W) -> FramedIo
    where
        W: AsyncWrite + Unpin + Send + Sync + 'static,
    {
        FramedIo::new(Box::new(futures::io::empty()), Box::new(writer))
    }

    struct FailingWrite;

    impl AsyncWrite for FailingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::Error::new(ErrorKind::BrokenPipe, "test failure")))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct PendingWrite;

    impl AsyncWrite for PendingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct FlushPendingWrite;

    impl AsyncWrite for FlushPendingWrite {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}
