use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::ZmqResult;
use crate::message::*;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::write_queue::write_message_queue;
use crate::{async_rt, CaptureSocket, SocketOptions};
use crate::{MultiPeerBackend, Socket, SocketBackend, SocketEvent, SocketSend, SocketType};

use async_trait::async_trait;
use bytes::Bytes;
use crossbeam_queue::ArrayQueue;
use futures::channel::{mpsc, oneshot};
use futures::{select, FutureExt, StreamExt};
use parking_lot::Mutex;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

const PUB_SEND_QUEUE_CAPACITY: usize = 100_000;
type PubSendQueue = mpsc::Sender<Message>;

fn subscription_matches(subscriptions: &[Vec<u8>], first_frame: &Bytes) -> bool {
    subscriptions.iter().any(|sub_filter| {
        sub_filter.len() <= first_frame.len()
            && sub_filter.as_slice() == &first_frame[0..sub_filter.len()]
    })
}

fn send_to_subscriber(mut send_queue: PubSendQueue, message: &ZmqMessage) {
    let message = Message::Message(message.clone());
    match send_queue.try_send(message) {
        Ok(()) => {}
        Err(error) if error.is_full() => {
            // PUB drops messages for slow subscribers so one full queue does not backpressure all publishers.
            drop(error.into_inner());
        }
        Err(error) => {
            // The writer task owns transport error detection and subscriber cleanup.
            // A closed local queue is treated as a dropped PUB message on this send path.
            drop(error.into_inner());
        }
    }
}

pub(crate) struct Subscriber {
    pub(crate) subscriptions: Vec<Vec<u8>>,
    pub(crate) send_queue: PubSendQueue,
    _subscription_coro_stop: oneshot::Sender<()>,
}

pub(crate) struct PubSocketBackend {
    subscribers: scc::HashMap<PeerIdentity, Subscriber>,
    subscriber_count: AtomicUsize,
    socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
    socket_options: SocketOptions,
}

struct PubFanoutQueue {
    queue: Arc<ArrayQueue<ZmqMessage>>,
    wake_sender: mpsc::UnboundedSender<()>,
    wake_pending: Arc<AtomicBool>,
}

impl PubFanoutQueue {
    fn publish(&self, message: ZmqMessage) -> ZmqResult<()> {
        if self.queue.push(message).is_err() {
            return Ok(());
        }

        if !self.wake_pending.load(Ordering::Acquire)
            && !self.wake_pending.swap(true, Ordering::AcqRel)
        {
            let _ = self.wake_sender.unbounded_send(());
        }

        Ok(())
    }
}

fn spawn_pub_fanout_queue(backend: Arc<PubSocketBackend>) -> PubFanoutQueue {
    let queue = Arc::new(ArrayQueue::new(PUB_SEND_QUEUE_CAPACITY));
    let wake_pending = Arc::new(AtomicBool::new(false));
    let (wake_sender, mut wake_receiver) = mpsc::unbounded();
    let worker_queue = queue.clone();
    let worker_pending = wake_pending.clone();

    async_rt::task::spawn(async move {
        while wake_receiver.next().await.is_some() {
            loop {
                while let Some(message) = worker_queue.pop() {
                    backend.fanout_message(message).await;
                }

                worker_pending.store(false, Ordering::Release);
                if worker_queue.is_empty() {
                    break;
                }

                // New messages arrived between draining and sleeping, so this task keeps ownership of the batch.
                worker_pending.store(true, Ordering::Release);
            }
        }
    });

    PubFanoutQueue {
        queue,
        wake_sender,
        wake_pending,
    }
}

impl PubSocketBackend {
    async fn fanout_message(&self, message: ZmqMessage) {
        let first_frame = match message.get(0) {
            Some(frame) => frame,
            None => return,
        };

        let mut iter = self.subscribers.begin_async().await;
        while let Some(subscriber) = iter {
            if subscription_matches(&subscriber.subscriptions, first_frame) {
                send_to_subscriber(subscriber.send_queue.clone(), &message);
            }
            iter = subscriber.next_async().await;
        }
    }

    fn message_received(&self, peer_id: &PeerIdentity, message: Message) {
        let data = match message {
            Message::Message(m) => {
                if m.len() != 1 {
                    log::warn!("Received message with unexpected length: {}", m.len());
                    return;
                }
                m.into_vec().pop().unwrap_or_default()
            }
            _ => return,
        };

        if data.is_empty() {
            return;
        }

        match data.first() {
            Some(1) => {
                // Subscribe
                if let Some(mut entry) = self.subscribers.get_sync(peer_id) {
                    entry.subscriptions.push(Vec::from(&data[1..]));
                }
            }
            Some(0) => {
                // Unsubscribe
                let sub = Vec::from(&data[1..]);
                if let Some(mut entry) = self.subscribers.get_sync(peer_id) {
                    if let Some(index) = entry.subscriptions.iter().position(|s| s == &sub) {
                        entry.subscriptions.remove(index);
                    }
                }
            }
            _ => log::warn!(
                "Received message with unexpected first byte: {:?}",
                data.first()
            ),
        }
    }
}

impl SocketBackend for PubSocketBackend {
    fn socket_type(&self) -> SocketType {
        SocketType::PUB
    }

    fn socket_options(&self) -> &SocketOptions {
        &self.socket_options
    }

    fn shutdown(&self) {
        self.subscribers.clear_sync();
        self.subscriber_count.store(0, Ordering::Relaxed);
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

#[async_trait]
impl MultiPeerBackend for PubSocketBackend {
    async fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        let (mut recv_queue, send_queue) = io.into_parts();
        // TODO provide handling for recv_queue
        let (queue_sender, queue_receiver) = mpsc::channel(PUB_SEND_QUEUE_CAPACITY);
        let (sender, stop_receiver) = oneshot::channel();
        let old_subscriber = self
            .subscribers
            .upsert_async(
                peer_id.clone(),
                Subscriber {
                    subscriptions: vec![],
                    send_queue: queue_sender,
                    _subscription_coro_stop: sender,
                },
            )
            .await;
        if old_subscriber.is_none() {
            self.subscriber_count.fetch_add(1, Ordering::Relaxed);
        }
        let writer_backend = self.clone();
        let writer_peer_id = peer_id.clone();
        async_rt::task::spawn(async move {
            if let Err(error) = write_message_queue(queue_receiver, send_queue).await {
                log::debug!(
                    "Error sending message to subscriber {:?}: {:?}",
                    writer_peer_id,
                    error
                );
                writer_backend.peer_disconnected(&writer_peer_id);
            }
        });
        let backend = self;
        let peer_id = peer_id.clone();
        async_rt::task::spawn(async move {
            let mut stop_receiver = stop_receiver.fuse();
            loop {
                select! {
                     _ = stop_receiver => {
                         break;
                     },
                     message = recv_queue.next().fuse() => {
                        match message {
                            Some(Ok(m)) => backend.message_received(&peer_id, m),
                            Some(Err(e)) => {
                                log::debug!("Error receiving message: {:?}", e);
                                backend.peer_disconnected(&peer_id);
                                break;
                            }
                            None => {
                                backend.peer_disconnected(&peer_id);
                                break
                            }
                        }

                     }
                }
            }
        });
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        log::info!("Client disconnected {:?}", peer_id);
        if let Some(monitor) = self.monitor().lock().as_mut() {
            let _ = monitor.try_send(SocketEvent::Disconnected(peer_id.clone()));
        }
        if self.subscribers.remove_sync(peer_id).is_some() {
            self.subscriber_count.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

pub struct PubSocket {
    pub(crate) backend: Arc<PubSocketBackend>,
    fanout_queue: PubFanoutQueue,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for PubSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl SocketSend for PubSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        if message.get(0).is_none() {
            return Ok(());
        }

        if self.backend.subscriber_count.load(Ordering::Relaxed) == 0 {
            return Ok(());
        }

        self.fanout_queue.publish(message)
    }
}

impl CaptureSocket for PubSocket {}

#[async_trait]
impl Socket for PubSocket {
    fn with_options(options: SocketOptions) -> Self {
        let backend = Arc::new(PubSocketBackend {
            subscribers: scc::HashMap::new(),
            subscriber_count: AtomicUsize::new(0),
            socket_monitor: Mutex::new(None),
            socket_options: options,
        });
        let fanout_queue = spawn_pub_fanout_queue(backend.clone());

        Self {
            backend,
            fanout_queue,
            binds: HashMap::new(),
        }
    }

    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }

    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle> {
        &mut self.binds
    }

    fn monitor(&mut self) -> mpsc::Receiver<SocketEvent> {
        let (sender, receiver) = mpsc::channel(1024);
        self.backend.socket_monitor.lock().replace(sender);
        receiver
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::tests::{
        test_bind_to_any_port_helper, test_bind_to_unspecified_interface_helper,
    };
    use crate::ZmqResult;
    use std::net::IpAddr;

    fn test_backend() -> PubSocketBackend {
        PubSocketBackend {
            subscribers: scc::HashMap::new(),
            subscriber_count: AtomicUsize::new(0),
            socket_monitor: Mutex::new(None),
            socket_options: SocketOptions::default(),
        }
    }

    async fn insert_test_subscriber(
        backend: &PubSocketBackend,
        peer_id: PeerIdentity,
        subscriptions: Vec<Vec<u8>>,
        queue_sender: PubSendQueue,
    ) {
        let (stop_sender, _stop_receiver) = oneshot::channel();
        let old_subscriber = backend
            .subscribers
            .upsert_async(
                peer_id,
                Subscriber {
                    subscriptions,
                    send_queue: queue_sender,
                    _subscription_coro_stop: stop_sender,
                },
            )
            .await;
        if old_subscriber.is_none() {
            backend.subscriber_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn test_fanout_queue(capacity: usize) -> (PubFanoutQueue, mpsc::UnboundedReceiver<()>) {
        let (wake_sender, wake_receiver) = mpsc::unbounded();
        let fanout_queue = PubFanoutQueue {
            queue: Arc::new(ArrayQueue::new(capacity)),
            wake_sender,
            wake_pending: Arc::new(AtomicBool::new(false)),
        };
        (fanout_queue, wake_receiver)
    }

    fn queued_frame(message: ZmqMessage) -> Bytes {
        message.get(0).expect("queued frame").clone()
    }

    #[test]
    fn test_fanout_queue_drops_new_message_when_full_without_extra_wake() {
        let (fanout_queue, mut wake_receiver) = test_fanout_queue(1);

        fanout_queue
            .publish(ZmqMessage::from(Bytes::from_static(b"first")))
            .expect("publish first message");

        assert_eq!(fanout_queue.queue.len(), 1);
        assert!(fanout_queue.wake_pending.load(Ordering::Relaxed));
        wake_receiver.try_recv().expect("first wake");

        fanout_queue
            .publish(ZmqMessage::from(Bytes::from_static(b"dropped")))
            .expect("publish dropped message");

        assert_eq!(fanout_queue.queue.len(), 1);
        assert!(wake_receiver.try_recv().is_err());
        assert_eq!(
            queued_frame(fanout_queue.queue.pop().expect("queued message")),
            Bytes::from_static(b"first")
        );
    }

    #[test]
    fn test_fanout_queue_wakes_again_after_pending_is_cleared() {
        let (fanout_queue, mut wake_receiver) = test_fanout_queue(2);

        fanout_queue
            .publish(ZmqMessage::from(Bytes::from_static(b"first")))
            .expect("publish first message");
        wake_receiver.try_recv().expect("first wake");
        assert!(fanout_queue.queue.pop().is_some());

        // Simulate the worker releasing pending after draining the current batch; the next publish must wake it again.
        fanout_queue.wake_pending.store(false, Ordering::Release);

        fanout_queue
            .publish(ZmqMessage::from(Bytes::from_static(b"second")))
            .expect("publish second message");

        wake_receiver.try_recv().expect("second wake");
        assert_eq!(
            queued_frame(fanout_queue.queue.pop().expect("queued message")),
            Bytes::from_static(b"second")
        );
    }

    #[async_rt::test]
    async fn test_send_without_subscribers_does_not_enqueue_fanout_message() {
        let mut socket = PubSocket::new();

        socket
            .send(ZmqMessage::from("dropped without subscribers"))
            .await
            .expect("send without subscribers");

        assert!(socket.fanout_queue.queue.is_empty());
    }

    #[async_rt::test]
    async fn test_replacing_subscriber_does_not_increment_subscriber_count() {
        let backend = test_backend();
        let peer_id = PeerIdentity::new();
        let (first_sender, _first_receiver) = mpsc::channel(8);
        let (second_sender, _second_receiver) = mpsc::channel(8);

        insert_test_subscriber(&backend, peer_id.clone(), vec![vec![]], first_sender).await;
        insert_test_subscriber(
            &backend,
            peer_id.clone(),
            vec![b"topic".to_vec()],
            second_sender,
        )
        .await;

        assert_eq!(backend.subscriber_count.load(Ordering::Relaxed), 1);
        assert_eq!(backend.subscribers.len(), 1);
    }

    #[async_rt::test]
    async fn test_shutdown_clears_subscriber_count() {
        let backend = test_backend();
        let (sender, _receiver) = mpsc::channel(8);

        insert_test_subscriber(&backend, PeerIdentity::new(), vec![vec![]], sender).await;

        assert_eq!(backend.subscriber_count.load(Ordering::Relaxed), 1);

        backend.shutdown();

        assert_eq!(backend.subscriber_count.load(Ordering::Relaxed), 0);
        assert!(backend.subscribers.is_empty());
    }

    #[async_rt::test]
    async fn test_fanout_message_delivers_only_matching_subscribers() {
        let backend = test_backend();
        let matching_peer = PeerIdentity::new();
        let non_matching_peer = PeerIdentity::new();
        let (matching_sender, mut matching_receiver) = mpsc::channel(8);
        let (non_matching_sender, mut non_matching_receiver) = mpsc::channel(8);

        insert_test_subscriber(
            &backend,
            matching_peer,
            vec![b"topic.a".to_vec()],
            matching_sender,
        )
        .await;
        insert_test_subscriber(
            &backend,
            non_matching_peer,
            vec![b"topic.b".to_vec()],
            non_matching_sender,
        )
        .await;

        backend
            .fanout_message(ZmqMessage::from(Bytes::from_static(b"topic.a payload")))
            .await;

        let matched = matching_receiver.try_recv().expect("matching subscriber");
        let Message::Message(matched) = matched else {
            panic!("unexpected queued message type");
        };
        assert_eq!(
            matched.get(0),
            Some(&Bytes::from_static(b"topic.a payload"))
        );
        assert!(non_matching_receiver.try_recv().is_err());
    }

    #[async_rt::test]
    async fn test_closed_subscriber_queue_does_not_interrupt_fanout_to_matching_peer() {
        let backend = test_backend();
        let stale_peer = PeerIdentity::new();
        let live_peer = PeerIdentity::new();
        let (stale_sender, stale_receiver) = mpsc::channel(1);
        let (live_sender, mut live_receiver) = mpsc::channel(8);
        drop(stale_receiver);

        insert_test_subscriber(&backend, stale_peer.clone(), vec![vec![]], stale_sender).await;
        insert_test_subscriber(&backend, live_peer.clone(), vec![vec![]], live_sender).await;

        backend
            .fanout_message(ZmqMessage::from(Bytes::from_static(b"payload")))
            .await;

        let queued = live_receiver.try_recv().expect("live subscriber message");
        let Message::Message(message) = queued else {
            panic!("unexpected queued message type");
        };
        assert_eq!(message.get(0), Some(&Bytes::from_static(b"payload")));

        assert!(backend.subscribers.get_sync(&stale_peer).is_some());
        assert!(backend.subscribers.get_sync(&live_peer).is_some());
        assert_eq!(backend.subscriber_count.load(Ordering::Relaxed), 2);
    }

    #[async_rt::test]
    async fn test_full_subscriber_queue_drops_message_without_disconnect() {
        let backend = test_backend();
        let peer_id = PeerIdentity::new();
        let (mut queue_sender, mut queue_receiver) = mpsc::channel(1);

        insert_test_subscriber(
            &backend,
            peer_id.clone(),
            vec![vec![]],
            queue_sender.clone(),
        )
        .await;

        let mut prefilled_count = 0;
        loop {
            let message = Message::Message(ZmqMessage::from(format!(
                "already queued {prefilled_count}"
            )));
            match queue_sender.try_send(message) {
                Ok(()) => prefilled_count += 1,
                Err(error) if error.is_full() => {
                    drop(error.into_inner());
                    break;
                }
                Err(error) => panic!("unexpected queue prefill error: {error:?}"),
            }
        }
        assert!(prefilled_count > 0);

        send_to_subscriber(queue_sender, &ZmqMessage::from("dropped payload"));

        assert!(backend.subscribers.get_sync(&peer_id).is_some());
        assert_eq!(backend.subscriber_count.load(Ordering::Relaxed), 1);

        let mut queued_messages = Vec::new();
        while let Ok(queued) = queue_receiver.try_recv() {
            let Message::Message(message) = queued else {
                panic!("unexpected queued message type");
            };
            queued_messages.push(String::try_from(message).unwrap());
        }

        assert_eq!(queued_messages.len(), prefilled_count);
        assert!(queued_messages
            .iter()
            .all(|message| message.starts_with("already queued ")));
        assert!(!queued_messages
            .iter()
            .any(|message| message == "dropped payload"));
    }

    #[async_rt::test]
    async fn test_send_to_subscriber_drops_closed_queue_without_disconnect() {
        let backend = test_backend();
        let peer_id = PeerIdentity::new();
        let (queue_sender, queue_receiver) = mpsc::channel(1);
        drop(queue_receiver);

        insert_test_subscriber(
            &backend,
            peer_id.clone(),
            vec![vec![]],
            queue_sender.clone(),
        )
        .await;

        send_to_subscriber(queue_sender, &ZmqMessage::from("payload"));

        assert!(backend.subscribers.get_sync(&peer_id).is_some());
        assert_eq!(backend.subscriber_count.load(Ordering::Relaxed), 1);
    }

    #[async_rt::test]
    async fn test_bind_to_any_port() -> ZmqResult<()> {
        let s = PubSocket::new();
        test_bind_to_any_port_helper(s).await
    }

    #[async_rt::test]
    async fn test_bind_to_any_ipv4_interface() -> ZmqResult<()> {
        let any_ipv4: IpAddr = "0.0.0.0".parse().unwrap();
        let s = PubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv4, s, 4000).await
    }

    #[async_rt::test]
    async fn test_bind_to_any_ipv6_interface() -> ZmqResult<()> {
        let any_ipv6: IpAddr = "::".parse().unwrap();
        let s = PubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv6, s, 4010).await
    }
}
