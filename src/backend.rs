use crate::async_rt;
use crate::codec::{FramedIo, Message, ZmqFramedRead};
use crate::fair_queue::QueueInner;
use crate::util::PeerIdentity;
use crate::write_queue::write_message_queue;
use crate::{
    MultiPeerBackend, SocketBackend, SocketEvent, SocketOptions, SocketType, ZmqError, ZmqResult,
};

use async_trait::async_trait;
use crossbeam_queue::SegQueue;
use futures::channel::mpsc;
use futures::SinkExt;
use parking_lot::Mutex;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Sender for notifying reconnection tasks when a peer disconnects.
pub(crate) type DisconnectNotifier = mpsc::Sender<PeerIdentity>;
const PEER_SEND_QUEUE_CAPACITY: usize = 100_000;

pub(crate) struct Peer {
    pub(crate) send_queue: mpsc::Sender<Message>,
}

#[derive(Clone)]
struct SinglePeer {
    peer_id: PeerIdentity,
    send_queue: mpsc::Sender<Message>,
}

enum CachedSend {
    Sent,
    Full {
        peer_id: PeerIdentity,
        send_queue: mpsc::Sender<Message>,
        message: Message,
    },
    Disconnected {
        peer_id: PeerIdentity,
        error: mpsc::SendError,
    },
    Miss(Message),
}

enum SendResolution {
    Sent,
    Miss(Message),
}

pub(crate) struct GenericSocketBackend {
    pub(crate) peers: scc::HashMap<PeerIdentity, Peer>,
    fair_queue_inner: Option<Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>>,
    pub(crate) round_robin: SegQueue<PeerIdentity>,
    socket_type: SocketType,
    socket_options: SocketOptions,
    pub(crate) socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
    single_peer: Mutex<Option<SinglePeer>>,
    peer_count: AtomicUsize,
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
            single_peer: Mutex::new(None),
            peer_count: AtomicUsize::new(0),
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

    pub(crate) async fn send_round_robin(&self, message: Message) -> ZmqResult<()> {
        let message = match self
            .finish_cached_send(self.try_send_cached_single_peer(message, None))
            .await?
        {
            SendResolution::Sent => return Ok(()),
            SendResolution::Miss(message) => message,
        };

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
            let send_result = match self.peers.get_async(&next_peer_id).await {
                Some(mut peer) => send_to_peer_queue(&mut peer.send_queue, message).await,
                None => continue,
            };
            return match send_result {
                Ok(()) => {
                    self.round_robin.push(next_peer_id);
                    Ok(())
                }
                Err(e) => {
                    self.peer_disconnected(&next_peer_id);
                    Err(e.into())
                }
            };
        }
    }

    pub(crate) async fn send_to_peer(
        &self,
        peer_id: &PeerIdentity,
        message: Message,
    ) -> ZmqResult<()> {
        let message = match self
            .finish_cached_send(self.try_send_cached_single_peer(message, Some(peer_id)))
            .await?
        {
            SendResolution::Sent => return Ok(()),
            SendResolution::Miss(message) => message,
        };

        let send_result = match self.peers.get_async(peer_id).await {
            Some(mut peer) => send_to_peer_queue(&mut peer.send_queue, message).await,
            None => return Err(ZmqError::Other("Destination client not found by identity")),
        };
        match send_result {
            Ok(()) => Ok(()),
            Err(e) => {
                self.peer_disconnected(peer_id);
                Err(e.into())
            }
        }
    }

    fn try_send_cached_single_peer(
        &self,
        message: Message,
        expected_peer_id: Option<&PeerIdentity>,
    ) -> CachedSend {
        if self.peer_count.load(Ordering::Acquire) != 1 {
            return CachedSend::Miss(message);
        }

        let mut single_peer = self.single_peer.lock();
        let Some(single_peer) = single_peer.as_mut() else {
            return CachedSend::Miss(message);
        };

        if let Some(expected_peer_id) = expected_peer_id {
            if single_peer.peer_id != *expected_peer_id {
                return CachedSend::Miss(message);
            }
        }

        match single_peer.send_queue.try_send(message) {
            Ok(()) => CachedSend::Sent,
            Err(error) if error.is_full() => CachedSend::Full {
                peer_id: single_peer.peer_id.clone(),
                send_queue: single_peer.send_queue.clone(),
                message: error.into_inner(),
            },
            Err(error) => CachedSend::Disconnected {
                peer_id: single_peer.peer_id.clone(),
                error: error.into_send_error(),
            },
        }
    }

    async fn finish_cached_send(&self, cached_send: CachedSend) -> ZmqResult<SendResolution> {
        match cached_send {
            CachedSend::Sent => Ok(SendResolution::Sent),
            CachedSend::Full {
                peer_id,
                mut send_queue,
                message,
            } => match send_queue.send(message).await {
                Ok(()) => Ok(SendResolution::Sent),
                Err(e) => {
                    self.peer_disconnected(&peer_id);
                    Err(e.into())
                }
            },
            CachedSend::Disconnected { peer_id, error } => {
                self.peer_disconnected(&peer_id);
                Err(error.into())
            }
            CachedSend::Miss(message) => Ok(SendResolution::Miss(message)),
        }
    }

    fn refresh_single_peer_cache(&self) {
        let peer_count = self.peers.len();
        self.peer_count.store(peer_count, Ordering::Release);

        let mut single_peer = self.single_peer.lock();
        if peer_count == 1 {
            *single_peer = self.peers.begin_sync().map(|peer| SinglePeer {
                peer_id: peer.key().clone(),
                send_queue: peer.send_queue.clone(),
            });
        } else {
            *single_peer = None;
        }
    }
}

async fn send_to_peer_queue(
    send_queue: &mut mpsc::Sender<Message>,
    message: Message,
) -> Result<(), mpsc::SendError> {
    match send_queue.try_send(message) {
        Ok(()) => Ok(()),
        Err(error) if error.is_full() => send_queue.send(error.into_inner()).await,
        Err(error) => Err(error.into_send_error()),
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
        self.single_peer.lock().take();
        self.peer_count.store(0, Ordering::Release);
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
        let (queue_sender, queue_receiver) = mpsc::channel(PEER_SEND_QUEUE_CAPACITY);
        self.peers
            .upsert_async(
                peer_id.clone(),
                Peer {
                    send_queue: queue_sender,
                },
            )
            .await;
        self.refresh_single_peer_cache();
        let writer_backend = self.clone();
        let writer_peer_id = peer_id.clone();
        async_rt::task::spawn(async move {
            if let Err(error) = write_message_queue(queue_receiver, send_queue).await {
                log::debug!(
                    "Error sending message to peer {:?}: {:?}",
                    writer_peer_id,
                    error
                );
                writer_backend.peer_disconnected(&writer_peer_id);
            }
        });
        self.round_robin.push(peer_id.clone());
        match &self.fair_queue_inner {
            None => {}
            Some(inner) => {
                inner.lock().insert(peer_id.clone(), recv_queue);
            }
        };
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove_sync(peer_id);
        self.refresh_single_peer_cache();
        match &self.fair_queue_inner {
            None => {}
            Some(inner) => {
                inner.lock().remove(peer_id);
            }
        };

        // Notify reconnection task if registered
        if let Some(mut notifier) = self.disconnect_notifiers.lock().remove(peer_id) {
            // Use try_send to avoid blocking - if channel is full, the reconnect task
            // will eventually notice the peer is gone
            let _ = notifier.try_send(peer_id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ZmqMessage;

    use bytes::Bytes;
    use futures::StreamExt;

    fn message_with_frame(frame: &'static [u8]) -> Message {
        Message::Message(ZmqMessage::from(Bytes::from_static(frame)))
    }

    fn first_frame(message: Message) -> Bytes {
        let Message::Message(message) = message else {
            panic!("unexpected queued message type");
        };
        message.get(0).expect("message frame").clone()
    }

    async fn insert_peer(
        backend: &GenericSocketBackend,
        peer_id: PeerIdentity,
    ) -> (PeerIdentity, mpsc::Receiver<Message>) {
        let (send_queue, recv_queue) = mpsc::channel(8);
        backend
            .peers
            .upsert_async(peer_id.clone(), Peer { send_queue })
            .await;
        backend.refresh_single_peer_cache();
        backend.round_robin.push(peer_id.clone());
        (peer_id, recv_queue)
    }

    #[crate::async_rt::test]
    async fn test_send_round_robin_uses_single_peer_cache() {
        let backend =
            GenericSocketBackend::with_options(None, SocketType::PUSH, SocketOptions::default());
        let (_peer_id, mut recv_queue) =
            insert_peer(&backend, "peer-a".parse().expect("peer id")).await;

        assert_eq!(1, backend.peer_count.load(Ordering::Acquire));
        assert!(backend.single_peer.lock().is_some());

        backend
            .send_round_robin(message_with_frame(b"payload"))
            .await
            .expect("send through single peer cache");

        let queued = recv_queue.next().await.expect("queued message");
        assert_eq!(Bytes::from_static(b"payload"), first_frame(queued));
    }

    #[crate::async_rt::test]
    async fn test_send_to_peer_uses_single_peer_cache_for_matching_identity() {
        let backend =
            GenericSocketBackend::with_options(None, SocketType::ROUTER, SocketOptions::default());
        let (peer_id, mut recv_queue) =
            insert_peer(&backend, "router-peer".parse().expect("peer id")).await;

        assert_eq!(
            Some(peer_id.clone()),
            backend
                .single_peer
                .lock()
                .as_ref()
                .map(|peer| peer.peer_id.clone())
        );

        backend
            .send_to_peer(&peer_id, message_with_frame(b"routed"))
            .await
            .expect("send to cached peer");

        let queued = recv_queue.next().await.expect("queued message");
        assert_eq!(Bytes::from_static(b"routed"), first_frame(queued));
    }

    #[crate::async_rt::test]
    async fn test_send_to_peer_routes_by_identity_with_multiple_peers() {
        let backend =
            GenericSocketBackend::with_options(None, SocketType::ROUTER, SocketOptions::default());
        let (_first_peer_id, mut first_recv_queue) =
            insert_peer(&backend, "router-peer-a".parse().expect("peer id")).await;
        let (second_peer_id, mut second_recv_queue) =
            insert_peer(&backend, "router-peer-b".parse().expect("peer id")).await;

        assert_eq!(2, backend.peer_count.load(Ordering::Acquire));
        assert!(backend.single_peer.lock().is_none());

        backend
            .send_to_peer(&second_peer_id, message_with_frame(b"routed-b"))
            .await
            .expect("send to selected peer");

        assert!(first_recv_queue.try_recv().is_err());
        let queued = second_recv_queue.next().await.expect("queued message");
        assert_eq!(Bytes::from_static(b"routed-b"), first_frame(queued));
    }

    #[crate::async_rt::test]
    async fn test_send_round_robin_waits_when_cached_sender_is_full() {
        let backend =
            GenericSocketBackend::with_options(None, SocketType::PUSH, SocketOptions::default());
        let peer_id: PeerIdentity = "full-peer".parse().expect("peer id");
        let (mut send_queue, mut recv_queue) = mpsc::channel(1);
        send_queue
            .try_send(message_with_frame(b"prefilled"))
            .expect("prefill peer queue");
        backend
            .peers
            .upsert_async(peer_id.clone(), Peer { send_queue })
            .await;
        backend.refresh_single_peer_cache();
        backend.round_robin.push(peer_id.clone());

        let send_future = backend.send_round_robin(message_with_frame(b"after-full"));
        let recv_future = async {
            let first = recv_queue.next().await.expect("prefilled message");
            let second = recv_queue
                .next()
                .await
                .expect("message sent after capacity frees");
            (first, second)
        };

        let (send_result, (first, second)) = futures::join!(send_future, recv_future);

        send_result.expect("send waits for cached sender capacity");
        assert_eq!(Bytes::from_static(b"prefilled"), first_frame(first));
        assert_eq!(Bytes::from_static(b"after-full"), first_frame(second));
    }

    #[crate::async_rt::test]
    async fn test_single_peer_cache_is_cleared_when_cached_sender_is_disconnected() {
        let backend =
            GenericSocketBackend::with_options(None, SocketType::PUSH, SocketOptions::default());
        let peer_id: PeerIdentity = "closed-peer".parse().expect("peer id");
        let (send_queue, recv_queue) = mpsc::channel(1);
        drop(recv_queue);
        backend
            .peers
            .upsert_async(peer_id.clone(), Peer { send_queue })
            .await;
        backend.refresh_single_peer_cache();
        backend.round_robin.push(peer_id.clone());

        let error = backend
            .send_round_robin(message_with_frame(b"lost"))
            .await
            .expect_err("closed cached sender should fail");

        assert!(matches!(error, ZmqError::BufferFull(_)));
        assert_eq!(0, backend.peer_count.load(Ordering::Acquire));
        assert!(backend.single_peer.lock().is_none());
        assert!(backend.peers.get_async(&peer_id).await.is_none());
    }
}
