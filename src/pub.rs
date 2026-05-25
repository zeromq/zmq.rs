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
use futures::channel::{mpsc, oneshot};
use futures::{select, FutureExt, StreamExt};
use parking_lot::Mutex;

use std::collections::HashMap;
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
    socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
    socket_options: SocketOptions,
}

impl PubSocketBackend {
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
        self.subscribers
            .upsert_async(
                peer_id.clone(),
                Subscriber {
                    subscriptions: vec![],
                    send_queue: queue_sender,
                    _subscription_coro_stop: sender,
                },
            )
            .await;
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
        self.subscribers.remove_sync(peer_id);
    }
}

pub struct PubSocket {
    pub(crate) backend: Arc<PubSocketBackend>,
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
        let first_frame = match message.get(0) {
            Some(frame) => frame,
            None => return Ok(()), // Empty message, nothing to publish
        };
        let mut iter = self.backend.subscribers.begin_async().await;
        while let Some(subscriber) = iter {
            if subscription_matches(&subscriber.subscriptions, first_frame) {
                send_to_subscriber(subscriber.send_queue.clone(), &message);
            }
            iter = subscriber.next_async().await;
        }
        Ok(())
    }
}

impl CaptureSocket for PubSocket {}

#[async_trait]
impl Socket for PubSocket {
    fn with_options(options: SocketOptions) -> Self {
        Self {
            backend: Arc::new(PubSocketBackend {
                subscribers: scc::HashMap::new(),
                socket_monitor: Mutex::new(None),
                socket_options: options,
            }),
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
        backend
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
    }

    #[async_rt::test]
    async fn test_pub_send_queues_only_matching_subscribers() {
        let mut socket = PubSocket::new();
        let matching_peer = PeerIdentity::new();
        let non_matching_peer = PeerIdentity::new();
        let (matching_sender, mut matching_receiver) = mpsc::channel(8);
        let (non_matching_sender, mut non_matching_receiver) = mpsc::channel(8);

        insert_test_subscriber(
            socket.backend.as_ref(),
            matching_peer,
            vec![b"topic.a".to_vec()],
            matching_sender,
        )
        .await;
        insert_test_subscriber(
            socket.backend.as_ref(),
            non_matching_peer,
            vec![b"topic.b".to_vec()],
            non_matching_sender,
        )
        .await;

        <PubSocket as SocketSend>::send(
            &mut socket,
            ZmqMessage::from(Bytes::from_static(b"topic.a payload")),
        )
        .await
        .expect("publish message");

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
        let mut socket = PubSocket::new();
        let stale_peer = PeerIdentity::new();
        let live_peer = PeerIdentity::new();
        let (stale_sender, stale_receiver) = mpsc::channel(1);
        let (live_sender, mut live_receiver) = mpsc::channel(8);
        drop(stale_receiver);

        insert_test_subscriber(
            socket.backend.as_ref(),
            stale_peer.clone(),
            vec![vec![]],
            stale_sender,
        )
        .await;
        insert_test_subscriber(
            socket.backend.as_ref(),
            live_peer.clone(),
            vec![vec![]],
            live_sender,
        )
        .await;

        <PubSocket as SocketSend>::send(
            &mut socket,
            ZmqMessage::from(Bytes::from_static(b"payload")),
        )
        .await
        .expect("publish with stale subscriber");

        let queued = live_receiver.try_recv().expect("live subscriber message");
        let Message::Message(message) = queued else {
            panic!("unexpected queued message type");
        };
        assert_eq!(message.get(0), Some(&Bytes::from_static(b"payload")));

        assert!(socket.backend.subscribers.get_sync(&stale_peer).is_some());
        assert!(socket.backend.subscribers.get_sync(&live_peer).is_some());
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
