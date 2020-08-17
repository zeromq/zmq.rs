use crate::codec::*;
use crate::error::*;
use crate::fair_queue::FairQueue;
use crate::*;
use crate::{SocketType, ZmqResult};
use async_trait::async_trait;
use crossbeam::atomic::AtomicCell;
use crossbeam::queue::SegQueue;
use dashmap::DashMap;
use futures::lock::Mutex;
use futures::stream::FuturesUnordered;
use futures_util::sink::SinkExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::stream::StreamExt;

struct ReqSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, Peer>,
    pub(crate) round_robin: SegQueue<PeerIdentity>,
    pub(crate) current_request_peer_id: Mutex<Option<PeerIdentity>>,
}

pub struct ReqSocket {
    backend: Arc<ReqSocketBackend>,
    _accept_close_handle: Option<oneshot::Sender<bool>>,
    current_request: Option<PeerIdentity>,
}

#[async_trait]
impl BlockingSend for ReqSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        if self.current_request.is_some() {
            return Err(ZmqError::Socket(
                "Unable to send message. Request already in progress",
            ));
        }
        // In normal scenario this will always be only 1 iteration
        // There can be special case when peer has disconnected and his id is still in RR queue
        // This happens because SegQueue don't have an api to delete items from queue.
        // So in such case we'll just pop item and skip it if we don't have a matching peer in peers map
        loop {
            let next_peer_id = match self.backend.round_robin.pop() {
                Ok(peer) => peer,
                Err(_) => {
                    return Err(ZmqError::Other(
                        "Not connected to peers. Unable to send messages",
                    ))
                }
            };
            match self.backend.peers.get_mut(&next_peer_id) {
                Some(mut peer) => {
                    self.backend.round_robin.push(next_peer_id.clone());
                    let frames = vec![
                        "".into(), // delimiter frame
                        message,
                    ];
                    peer.send_queue
                        .send(Message::MultipartMessage(frames))
                        .await?;
                    self.backend
                        .current_request_peer_id
                        .lock()
                        .await
                        .replace(next_peer_id.clone());
                    self.current_request = Some(next_peer_id);
                    return Ok(());
                }
                None => continue,
            }
        }
    }
}

#[async_trait]
impl BlockingRecv for ReqSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        match self.current_request.take() {
            Some(peer_id) => {
                if let Some(recv_queue) = self
                    .backend
                    .peers
                    .get(&peer_id)
                    .map(|p| p.recv_queue.clone())
                {
                    let message = recv_queue.lock().await.next().await;
                    match message {
                        Some(Message::MultipartMessage(mut message)) => {
                            assert!(message.len() == 2);
                            assert!(message[0].data.is_empty()); // Ensure that we have delimeter as first part
                            Ok(message.pop().unwrap())
                        }
                        Some(_) => Err(ZmqError::Other("Wrong message type received")),
                        None => Err(ZmqError::NoMessage),
                    }
                } else {
                    Err(ZmqError::Other("Server disconnected"))
                }
            }
            None => Err(ZmqError::Other("Unable to recv. No request in progress")),
        }
    }
}

#[async_trait]
impl SocketFrontend for ReqSocket {
    fn new() -> Self {
        Self {
            backend: Arc::new(ReqSocketBackend {
                peers: DashMap::new(),
                round_robin: SegQueue::new(),
                current_request_peer_id: Mutex::new(None),
            }),
            _accept_close_handle: None,
            current_request: None,
        }
    }

    async fn bind(&mut self, endpoint: &str) -> ZmqResult<()> {
        if self._accept_close_handle.is_some() {
            return Err(ZmqError::Other(
                "Socket server already started. Currently only one server is supported",
            ));
        }
        let stop_handle = util::start_accepting_connections(endpoint, self.backend.clone()).await?;
        self._accept_close_handle = Some(stop_handle);
        Ok(())
    }

    async fn connect(&mut self, endpoint: &str) -> ZmqResult<()> {
        let addr = endpoint.parse::<SocketAddr>()?;
        let raw_socket = tokio::net::TcpStream::connect(addr).await?;
        util::peer_connected(raw_socket, self.backend.clone()).await;
        Ok(())
    }
}

#[async_trait]
impl MultiPeer for ReqSocketBackend {
    async fn peer_connected(
        &self,
        peer_id: &PeerIdentity,
    ) -> (mpsc::Receiver<Message>, oneshot::Receiver<bool>) {
        let default_queue_size = 1;
        let (out_queue, out_queue_receiver) = mpsc::channel(default_queue_size);
        let (in_queue, in_queue_receiver) = mpsc::channel(default_queue_size);
        let (stop_handle, stop_callback) = oneshot::channel::<bool>();

        self.peers.insert(
            peer_id.clone(),
            Peer {
                identity: peer_id.clone(),
                send_queue: out_queue,
                recv_queue: Arc::new(Mutex::new(in_queue_receiver)),
                recv_queue_in: in_queue,
                _io_close_handle: stop_handle,
            },
        );
        self.round_robin.push(peer_id.clone());

        (out_queue_receiver, stop_callback)
    }

    async fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }
}

#[async_trait]
impl SocketBackend for ReqSocketBackend {
    async fn message_received(&self, peer_id: &PeerIdentity, message: Message) {
        // This is needed to ensure that we only store messages that we are expecting to get
        // Other messages are silently discarded according to spec
        let mut curr_req_lock = self.current_request_peer_id.lock().await;
        match curr_req_lock.take() {
            Some(id) => {
                if &id != peer_id {
                    curr_req_lock.replace(id);
                    return;
                }
            }
            None => return,
        }
        drop(curr_req_lock);
        // We've got reply that we were waiting for
        self.peers
            .get_mut(peer_id)
            .expect("Not found peer by id")
            .recv_queue_in
            .send(message)
            .await
            .expect("Failed to send");
    }

    fn socket_type(&self) -> SocketType {
        SocketType::REQ
    }

    fn shutdown(&self) {
        self.peers.clear();
    }
}

struct RepPeer {
    pub(crate) identity: PeerIdentity,
    pub(crate) send_queue: mpsc::Sender<Message>,
    pub(crate) recv_queue_in: mpsc::Sender<Message>,
    pub(crate) _io_close_handle: futures::channel::oneshot::Sender<bool>,
}

struct FairQueueProcessor {
    pub(crate) fair_queue_stream: FairQueue<mpsc::Receiver<Message>, PeerIdentity>,
    pub(crate) socket_incoming_queue: mpsc::Sender<(PeerIdentity, Message)>,
    pub(crate) peer_queue_in:
        mpsc::Receiver<(PeerIdentity, mpsc::Receiver<Message>)>,
}

struct RepSocketBackend {
    pub(crate) peers: Arc<DashMap<PeerIdentity, RepPeer>>,
    pub(crate) processor: Mutex<FairQueueProcessor>,
    pub(crate) peer_queue_in: mpsc::Sender<(PeerIdentity, mpsc::Receiver<Message>)>,
}

pub struct RepSocket {
    backend: Arc<RepSocketBackend>,
    _accept_close_handle: Option<oneshot::Sender<bool>>,
    current_request: Option<PeerIdentity>,
    fair_queue: mpsc::Receiver<(PeerIdentity, Message)>,
}

#[async_trait]
impl SocketFrontend for RepSocket {
    fn new() -> Self {
        // TODO define buffer size
        let default_queue_size = 100;
        let (queue_sender, fair_queue) = mpsc::channel(default_queue_size);
        let (peer_in, peer_out) = mpsc::channel(default_queue_size);
        Self {
            backend: Arc::new(RepSocketBackend {
                peers: Arc::new(DashMap::new()),
                processor: Mutex::new(FairQueueProcessor {
                    fair_queue_stream: FairQueue::new(),
                    socket_incoming_queue: queue_sender,
                    peer_queue_in: peer_out,
                }),
                peer_queue_in: peer_in,
            }),
            _accept_close_handle: None,
            current_request: None,
            fair_queue,
        }
    }

    async fn bind(&mut self, endpoint: &str) -> ZmqResult<()> {
        let stop_handle = util::start_accepting_connections(endpoint, self.backend.clone()).await?;
        self._accept_close_handle = Some(stop_handle);
        Ok(())
    }

    async fn connect(&mut self, endpoint: &str) -> ZmqResult<()> {
        unimplemented!()
    }
}

#[async_trait]
impl MultiPeer for RepSocketBackend {
    async fn peer_connected(
        &self,
        peer_id: &PeerIdentity,
    ) -> (mpsc::Receiver<Message>, oneshot::Receiver<bool>) {
        let default_queue_size = 100;
        let (out_queue, out_queue_receiver) = mpsc::channel(default_queue_size);
        let (in_queue, in_queue_receiver) = mpsc::channel::<Message>(default_queue_size);
        let (stop_handle, stop_callback) = oneshot::channel::<bool>();

        self.peers.insert(
            peer_id.clone(),
            RepPeer {
                identity: peer_id.clone(),
                send_queue: out_queue,
                recv_queue_in: in_queue,
                _io_close_handle: stop_handle,
            },
        );
        self.peer_queue_in.clone().try_send(
            (peer_id.clone(), in_queue_receiver)
        ).unwrap();

        (out_queue_receiver, stop_callback)
    }

    async fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }
}

#[async_trait]
impl SocketBackend for RepSocketBackend {
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
        SocketType::REP
    }

    fn shutdown(&self) {
        self.peers.clear();
    }
}

#[async_trait]
impl BlockingSend for RepSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        match self.current_request.take() {
            Some(peer_id) => {
                if let Some(mut peer) = self.backend.peers.get_mut(&peer_id) {
                    let frames = vec![
                        "".into(), // delimiter frame
                        message,
                    ];
                    peer.send_queue
                        .send(Message::MultipartMessage(frames))
                        .await?;
                    Ok(())
                } else {
                    Err(ZmqError::Other("Client disconnected"))
                }
            }
            None => Err(ZmqError::Other(
                "Unable to send reply. No request in progress",
            )),
        }
    }
}

#[async_trait]
impl BlockingRecv for RepSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((peer_id, Message::MultipartMessage(mut messages))) => {
                    assert!(messages.len() == 2);
                    assert!(messages[0].data.is_empty()); // Ensure that we have delimeter as first part
                    self.current_request = Some(peer_id);
                    return Ok(messages.pop().unwrap());
                }
                Some((_peer_id, _)) => todo!(),
                None => todo!(),
            };
        }
    }
}
