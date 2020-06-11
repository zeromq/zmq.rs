use async_trait::async_trait;
use dashmap::DashMap;
use futures::channel::{mpsc, oneshot};
use futures::lock::Mutex;
use futures::SinkExt;
use std::convert::TryInto;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::message::*;
use crate::util::*;
use crate::{util, MultiPeer, SocketBackend, SocketFrontend};
use crate::{Socket, SocketType, ZmqResult};
use futures::stream::{FuturesUnordered, StreamExt};

struct Peer {
    pub(crate) identity: PeerIdentity,
    pub(crate) send_queue: mpsc::Sender<Message>,
    pub(crate) recv_queue: Arc<Mutex<mpsc::Receiver<Message>>>,
    pub(crate) recv_queue_in: mpsc::Sender<Message>,
    pub(crate) _io_close_handle: futures::channel::oneshot::Sender<bool>,
}

struct RouterSocketBackend {
    pub(crate) peers: Arc<DashMap<PeerIdentity, Peer>>,
}

impl MultiPeer for RouterSocketBackend {
    fn peer_connected(
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
                identity: peer_id.clone(),
                send_queue: out_queue,
                recv_queue: Arc::new(Mutex::new(in_queue_receiver)),
                recv_queue_in: in_queue,
                _io_close_handle: stop_handle,
            },
        );

        (out_queue_receiver, stop_callback)
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }
}

#[async_trait]
impl SocketBackend for RouterSocketBackend {
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
        SocketType::ROUTER
    }

    fn shutdown(&self) {
        self.peers.clear();
    }
}

/// A ZMQ `ROUTER` type socket.
pub struct RouterSocket {
    backend: Arc<RouterSocketBackend>,
    _accept_close_handle: Option<oneshot::Sender<bool>>,
}

impl Drop for RouterSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl SocketFrontend for RouterSocket {
    fn new() -> Self {
        Self {
            backend: Arc::new(RouterSocketBackend {
                peers: Arc::new(DashMap::new()),
            }),
            _accept_close_handle: None,
        }
    }

    async fn bind(&mut self, endpoint: &str) -> ZmqResult<()> {
        let stop_handle = util::start_accepting_connections(endpoint, self.backend.clone()).await?;
        self._accept_close_handle = Some(stop_handle);
        Ok(())
    }

    async fn connect(&mut self, _endpoint: &str) -> ZmqResult<()> {
        unimplemented!()
    }
}

impl RouterSocket {
    /// Receive a message in multipart.
    pub async fn recv_multipart(&mut self) -> ZmqResult<Vec<ZmqMessage>> {
        println!("Try recv multipart");
        let mut messages = FuturesUnordered::new();
        for peer in self.backend.peers.iter() {
            let peer_id = peer.identity.clone();
            let recv_queue = peer.recv_queue.clone();
            messages.push(async move { (peer_id, recv_queue.lock().await.next().await) });
        }
        loop {
            if messages.is_empty() {
                // TODO block to wait for connections
                return Err(ZmqError::NoMessage);
            }
            match messages.next().await {
                Some((peer_id, Some(Message::MultipartMessage(messages)))) => {
                    let mut envelope = vec![ZmqMessage {
                        data: peer_id.into(),
                    }];
                    envelope.extend(messages);
                    return Ok(envelope);
                }
                Some((peer_id, None)) => {
                    println!("Peer disconnected {:?}", peer_id);
                    self.backend.peers.remove(&peer_id);
                }
                Some((_peer_id, _)) => todo!(),
                None => continue,
            };
        }
    }

    /// Send a message in multipart.
    pub async fn send_multipart(&mut self, messages: Vec<ZmqMessage>) -> ZmqResult<()> {
        assert!(messages.len() > 2);
        let peer_id: PeerIdentity = messages[0].data.to_vec().try_into()?;
        match self.backend.peers.get_mut(&peer_id) {
            Some(mut peer) => {
                peer.send_queue
                    .try_send(Message::MultipartMessage(messages[1..].to_vec()))?;
                Ok(())
            }
            None => return Err(ZmqError::Other("Destination client not found by identity")),
        }
    }
}

/// A ZMQ `DEALER` type socket.
pub struct DealerSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

impl DealerSocket {
    /// Bind the socket to an endpoint.
    pub async fn bind(_endpoint: &str) -> ZmqResult<Self> {
        todo!()
    }
}

#[async_trait]
impl Socket for DealerSocket {
    async fn send(&mut self, _m: ZmqMessage) -> ZmqResult<()> {
        unimplemented!()
    }

    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        unimplemented!()
    }
}
