use async_trait::async_trait;
use dashmap::DashMap;
use futures::lock::Mutex;
use futures_util::sink::SinkExt;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::stream::StreamExt;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::util::raw_connect;
use crate::*;
use crate::{Socket, SocketType, ZmqResult};

struct ReqSocketBackend {
    pub(crate) peers: Arc<DashMap<PeerIdentity, Peer>>,
}

pub struct ReqSocket {
    backend: Arc<ReqSocketBackend>,
    _accept_close_handle: Option<oneshot::Sender<bool>>,
}

#[async_trait]
impl Socket for ReqSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        todo!()
        // let frames = vec![
        //     "".into(), // delimiter frame
        //     message,
        // ];
        // self._inner.send(Message::MultipartMessage(frames)).await
    }

    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        todo!()
        // match self._inner.next().await {
        //     Some(Ok(Message::MultipartMessage(mut message))) => {
        //         assert!(message.len() == 2);
        //         assert!(message[0].data.is_empty()); // Ensure that we have delimeter as first part
        //         Ok(message.pop().unwrap())
        //     }
        //     Some(Ok(_)) => Err(ZmqError::Other("Wrong message type received")),
        //     Some(Err(e)) => Err(e),
        //     None => Err(ZmqError::NoMessage),
        // }
    }
}

#[async_trait]
impl SocketFrontend for ReqSocket {
    fn new() -> Self {
        Self {
            backend: Arc::new(ReqSocketBackend {
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

    async fn connect(&mut self, endpoint: &str) -> ZmqResult<()> {
        let addr = endpoint.parse::<SocketAddr>()?;
        let raw_socket = tokio::net::TcpStream::connect(addr).await?;
        util::peer_connected(raw_socket, self.backend.clone());
        Ok(())
    }
}

impl MultiPeer for ReqSocketBackend {
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
impl SocketBackend for ReqSocketBackend {
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
        SocketType::REQ
    }

    fn shutdown(&self) {
        self.peers.clear();
    }
}

struct RepSocketBackend {
    pub(crate) peers: Arc<DashMap<PeerIdentity, Peer>>,
}

pub struct RepSocket {
    backend: Arc<RepSocketBackend>,
    _accept_close_handle: Option<oneshot::Sender<bool>>,
}

#[async_trait]
impl SocketFrontend for RepSocket {
    fn new() -> Self {
        Self {
            backend: Arc::new(RepSocketBackend {
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

    async fn connect(&mut self, endpoint: &str) -> ZmqResult<()> {
        unimplemented!()
    }
}

impl MultiPeer for RepSocketBackend {
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
impl Socket for RepSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        todo!()
        // let frames = vec![
        //     "".into(), // delimiter frame
        //     message,
        // ];
        // self._inner.send(Message::MultipartMessage(frames)).await
    }

    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        todo!()
        // match self._inner.next().await {
        //     Some(Ok(Message::MultipartMessage(mut message))) => {
        //         assert!(message.len() == 2);
        //         assert!(message[0].data.is_empty()); // Ensure that we have delimeter as first part
        //         Ok(message.pop().unwrap())
        //     }
        //     Some(Ok(_)) => Err(ZmqError::Other("Wrong message type received")),
        //     Some(Err(e)) => Err(e),
        //     None => Err(ZmqError::NoMessage),
        // }
    }
}
