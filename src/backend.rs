use crate::codec::{FrameableRead, FrameableWrite, FramedIo, Message, ZmqCodec};
use crate::fair_queue::FairQueue;
use crate::util::{FairQueueProcessor, PeerIdentity};
use crate::{util, MultiPeerBackend, SocketBackend, SocketType, ZmqError, ZmqResult};
use async_trait::async_trait;
use crossbeam::queue::SegQueue;
use dashmap::DashMap;
use futures::channel::{mpsc, oneshot};
use futures::SinkExt;
use futures_codec::{FramedRead, FramedWrite};

pub(crate) struct Peer {
    pub(crate) send_queue: FramedWrite<Box<dyn FrameableWrite>, ZmqCodec>,
}

pub(crate) struct GenericSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, Peer>,
    peer_queue_in: mpsc::Sender<(PeerIdentity, FramedRead<Box<dyn FrameableRead>, ZmqCodec>)>,
    _fair_queue_close_handle: oneshot::Sender<bool>,
    pub(crate) round_robin: SegQueue<PeerIdentity>,
    socket_type: SocketType,
}

impl GenericSocketBackend {
    pub(crate) fn new(
        queue_sender: mpsc::Sender<(PeerIdentity, Message)>,
        socket_type: SocketType,
    ) -> Self {
        // TODO define buffer size
        let default_queue_size = 100;
        let (peer_in, peer_out) = mpsc::channel(default_queue_size);
        let (fair_queue_close_handle, fqueue_close_recevier) = oneshot::channel();
        tokio::spawn(util::process_fair_queue_messages(FairQueueProcessor {
            fair_queue_stream: FairQueue::new(),
            socket_incoming_queue: queue_sender,
            peer_queue_in: peer_out,
            _io_close_handle: fqueue_close_recevier,
        }));
        Self {
            peers: DashMap::new(),
            peer_queue_in: peer_in,
            round_robin: SegQueue::new(),
            _fair_queue_close_handle: fair_queue_close_handle,
            socket_type,
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
                    Message::Multipart(m) => {
                        return Err(ZmqError::ReturnToSenderMultipart {
                            reason: "Not connected to peers. Unable to send messages",
                            messages: m,
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

#[async_trait]
impl SocketBackend for GenericSocketBackend {
    async fn message_received(&self, peer_id: &PeerIdentity, message: Message) {}

    fn socket_type(&self) -> SocketType {
        self.socket_type
    }

    fn shutdown(&self) {
        self.peers.clear();
    }
}

impl MultiPeerBackend for GenericSocketBackend {
    fn peer_connected(&self, peer_id: &PeerIdentity, io: FramedIo) {
        let (recv_queue, send_queue) = io.into_parts();
        self.peers.insert(peer_id.clone(), Peer { send_queue });
        self.round_robin.push(peer_id.clone());
        self.peer_queue_in
            .clone()
            .try_send((peer_id.clone(), recv_queue))
            .unwrap();
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }
}
