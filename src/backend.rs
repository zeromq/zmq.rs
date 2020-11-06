use crate::codec::Message;
use crate::fair_queue::FairQueue;
use crate::util::{FairQueueProcessor, PeerIdentity};
use crate::{util, MultiPeer, SocketBackend, SocketType};
use async_trait::async_trait;
use crossbeam::queue::SegQueue;
use dashmap::DashMap;
use futures::channel::{mpsc, oneshot};
use futures::SinkExt;

pub(crate) struct Peer {
    pub(crate) send_queue: mpsc::Sender<Message>,
    pub(crate) recv_queue_in: mpsc::Sender<Message>,
    pub(crate) _io_close_handle: futures::channel::oneshot::Sender<bool>,
}

pub(crate) struct GenericSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, Peer>,
    peer_queue_in: mpsc::Sender<(PeerIdentity, mpsc::Receiver<Message>)>,
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
}

#[async_trait]
impl SocketBackend for GenericSocketBackend {
    async fn message_received(&self, peer_id: &PeerIdentity, message: Message) {
        self.peers
            .get_mut(peer_id)
            .expect("Not found peer by id")
            .recv_queue_in
            .send(message)
            .await
            .expect("Failed to send");
    }

    fn socket_type(&self) -> SocketType {
        self.socket_type
    }

    fn shutdown(&self) {
        self.peers.clear();
    }
}

#[async_trait]
impl MultiPeer for GenericSocketBackend {
    async fn peer_connected(
        &self,
        peer_id: &PeerIdentity,
    ) -> (mpsc::Receiver<Message>, oneshot::Receiver<bool>) {
        let default_queue_size = 100;
        let (out_queue, out_queue_receiver) = mpsc::channel(default_queue_size);
        let (in_queue, in_queue_receiver) = mpsc::channel(default_queue_size);
        let (stop_handle, stop_callback) = oneshot::channel::<bool>();

        self.peers.insert(
            peer_id.clone(),
            Peer {
                send_queue: out_queue,
                recv_queue_in: in_queue,
                _io_close_handle: stop_handle,
            },
        );
        self.round_robin.push(peer_id.clone());
        self.peer_queue_in
            .clone()
            .try_send((peer_id.clone(), in_queue_receiver))
            .unwrap();

        (out_queue_receiver, stop_callback)
    }

    async fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }
}
