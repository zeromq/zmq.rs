use crate::codec::{FramedIo, Message, ZmqFramedRead, ZmqFramedWrite};
use crate::fair_queue::QueueInner;
use crate::util::PeerIdentity;
use crate::{MultiPeerBackend, SocketBackend, SocketEvent, SocketType, ZmqError, ZmqResult};
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
    pub(crate) socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
}

impl GenericSocketBackend {
    pub(crate) fn new(
        fair_queue_inner: Option<Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>>,
        socket_type: SocketType,
    ) -> Self {
        Self {
            peers: DashMap::new(),
            fair_queue_inner,
            round_robin: SegQueue::new(),
            socket_type,
            socket_monitor: Mutex::new(None),
        }
    }

    pub(crate) async fn send_round_robin(&self, message: Message) -> ZmqResult<PeerIdentity> {
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
            match self.peers.get_mut(&next_peer_id) {
                Some(mut peer) => {
                    let send_result = peer.send_queue.send(message).await;
                    match send_result {
                        Ok(()) => {
                            self.round_robin.push(next_peer_id.clone());
                            return Ok(next_peer_id);
                        }
                        Err(_) => {
                            panic!("Don't know how to handle this now");
                        }
                    };
                }
                None => continue,
            }
        }
    }
}

impl SocketBackend for GenericSocketBackend {
    fn socket_type(&self) -> SocketType {
        self.socket_type
    }

    fn shutdown(&self) {
        self.peers.clear();
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

impl MultiPeerBackend for GenericSocketBackend {
    fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        let (recv_queue, send_queue) = io.into_parts();
        self.peers.insert(peer_id.clone(), Peer { send_queue });
        self.round_robin.push(peer_id.clone());
        match &self.fair_queue_inner {
            None => {}
            Some(inner) => {
                inner.lock().insert(peer_id.clone(), recv_queue);
            }
        };
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }
}
