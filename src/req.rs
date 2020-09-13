use crate::codec::*;
use crate::error::*;
use crate::util::{self, Peer, PeerIdentity};
use crate::*;
use crate::{SocketType, ZmqResult};
use async_trait::async_trait;
use crossbeam::queue::SegQueue;
use dashmap::DashMap;
use futures::channel::{mpsc, oneshot};
use futures::lock::Mutex;
use futures_util::sink::SinkExt;
use std::sync::Arc;
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

impl Drop for ReqSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl BlockingSend for ReqSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        if self.current_request.is_some() {
            return Err(ZmqError::ReturnToSender {
                reason: "Unable to send message. Request already in progress",
                message,
            });
        }
        // In normal scenario this will always be only 1 iteration
        // There can be special case when peer has disconnected and his id is still in RR queue
        // This happens because SegQueue don't have an api to delete items from queue.
        // So in such case we'll just pop item and skip it if we don't have a matching peer in peers map
        loop {
            let next_peer_id = match self.backend.round_robin.pop() {
                Ok(peer) => peer,
                Err(_) => {
                    return Err(ZmqError::ReturnToSender {
                        reason: "Not connected to peers. Unable to send messages",
                        message,
                    })
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
impl Socket for ReqSocket {
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
        let curr_req_lock = self.current_request_peer_id.lock().await;
        match curr_req_lock.as_ref() {
            Some(id) => {
                if id != peer_id {
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
