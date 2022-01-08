use crate::codec::{FramedIo, Message, ZmqFramedRead, ZmqFramedWrite};
use crate::fair_queue::QueueInner;
use crate::util::{self, PeerIdentity};
use crate::{
    async_rt, Endpoint, MultiPeerBackend, SocketBackend, SocketEvent, SocketOptions, SocketType,
    ZmqError, ZmqResult,
};
use async_trait::async_trait;
use crossbeam::queue::SegQueue;
use dashmap::DashMap;
use futures::channel::mpsc;
use futures::SinkExt;
use parking_lot::Mutex;
use std::sync::Arc;

pub(crate) struct Peer {
    pub(crate) send_queue: ZmqFramedWrite,
}

pub(crate) struct GenericSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, Peer>,
    fair_queue_inner: Option<Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>>,
    pub(crate) round_robin: SegQueue<PeerIdentity>,
    socket_type: SocketType,
    socket_options: SocketOptions,
    pub(crate) socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
    connect_endpoints: DashMap<PeerIdentity, Endpoint>,
}

impl GenericSocketBackend {
    pub(crate) fn with_options(
        fair_queue_inner: Option<Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>>,
        socket_type: SocketType,
        options: SocketOptions,
    ) -> Self {
        Self {
            peers: DashMap::new(),
            fair_queue_inner,
            round_robin: SegQueue::new(),
            socket_type,
            socket_options: options,
            socket_monitor: Mutex::new(None),
            connect_endpoints: DashMap::new(),
        }
    }

    pub(crate) async fn send_round_robin(
        self: &Arc<Self>,
        message: Message,
    ) -> ZmqResult<PeerIdentity> {
        // In normal scenario this will always be only 1 iteration
        // There can be special case when peer has disconnected and his id is still in
        // RR queue This happens because SegQueue don't have an api to delete
        // items from queue. So in such case we'll just pop item and skip it if
        // we don't have a matching peer in peers map
        loop {
            let next_peer_id = match self.round_robin.pop() {
                Ok(peer) => peer,
                Err(_) => match message {
                    Message::Greeting(_) => panic!("Sending greeting is not supported"),
                    Message::Command(_) => panic!("Sending commands is not supported"),
                    Message::Message(m) => {
                        return Err(ZmqError::ReturnToSender {
                            reason: "Not connected to peers. Unable to send messages",
                            message: m,
                        })
                    }
                },
            };
            let send_result = match self.peers.get_mut(&next_peer_id) {
                Some(mut peer) => peer.send_queue.send(message).await,
                None => continue,
            };
            return match send_result {
                Ok(()) => {
                    self.round_robin.push(next_peer_id.clone());
                    Ok(next_peer_id)
                }
                Err(e) => {
                    self.clone().peer_disconnected(&next_peer_id);
                    Err(e.into())
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
        self.peers.clear();
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

#[async_trait]
impl MultiPeerBackend for GenericSocketBackend {
    async fn peer_connected(
        self: Arc<Self>,
        peer_id: &PeerIdentity,
        io: FramedIo,
        endpoint: Option<Endpoint>,
    ) {
        let (recv_queue, send_queue) = io.into_parts();
        self.peers.insert(peer_id.clone(), Peer { send_queue });
        self.round_robin.push(peer_id.clone());

        if let Some(queue_inner) = &self.fair_queue_inner {
            queue_inner.lock().insert(peer_id.clone(), recv_queue);
        }

        if let Some(e) = endpoint {
            self.connect_endpoints.insert(peer_id.clone(), e);
        }
    }

    fn peer_disconnected(self: Arc<Self>, peer_id: &PeerIdentity) {
        if let Some(monitor) = self.monitor().lock().as_mut() {
            let _ = monitor.try_send(SocketEvent::Disconnected(peer_id.clone()));
        }

        self.peers.remove(peer_id);
        if let Some(inner) = &self.fair_queue_inner {
            inner.lock().remove(peer_id);
        }

        let endpoint = match self.connect_endpoints.remove(peer_id) {
            Some((_, e)) => e,
            None => return,
        };
        let backend = self;

        async_rt::task::spawn(async move {
            let (socket, endpoint) = util::connect_forever(endpoint)
                .await
                .expect("Failed to connect");
            let peer_id = util::peer_connected(socket, backend.clone(), Some(endpoint.clone()))
                .await
                .expect("Failed to handshake");
            if let Some(monitor) = backend.monitor().lock().as_mut() {
                let _ = monitor.try_send(SocketEvent::Connected(endpoint, peer_id));
            }
        });
    }
}
