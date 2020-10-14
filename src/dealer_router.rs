use async_trait::async_trait;
use dashmap::DashMap;
use futures::channel::{mpsc, oneshot};
use futures::lock::Mutex;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::SinkExt;
use std::collections::HashMap;
use std::convert::TryInto;
use std::sync::Arc;

use crate::codec::FramedIo;
use crate::codec::*;
use crate::endpoint::{Endpoint, TryIntoEndpoint};
use crate::error::*;
use crate::message::*;
use crate::transport::{self, AcceptStopHandle};
use crate::util::{self, Peer, PeerIdentity};
use crate::{MultiPeer, Socket, SocketBackend};
use crate::{SocketType, ZmqResult};

struct RouterSocketBackend {
    pub(crate) peers: Arc<DashMap<PeerIdentity, Peer>>,
}

#[async_trait]
impl MultiPeer for RouterSocketBackend {
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
                identity: peer_id.clone(),
                send_queue: out_queue,
                recv_queue: Arc::new(Mutex::new(in_queue_receiver)),
                recv_queue_in: in_queue,
                _io_close_handle: stop_handle,
            },
        );

        (out_queue_receiver, stop_callback)
    }

    async fn peer_disconnected(&self, peer_id: &PeerIdentity) {
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

pub struct RouterSocket {
    backend: Arc<RouterSocketBackend>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for RouterSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl Socket for RouterSocket {
    fn new() -> Self {
        Self {
            backend: Arc::new(RouterSocketBackend {
                peers: Arc::new(DashMap::new()),
            }),
            binds: HashMap::new(),
        }
    }

    async fn bind(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) -> ZmqResult<Endpoint> {
        let endpoint = endpoint.try_into()?;

        let cloned_backend = self.backend.clone();
        let cback = move |result| util::peer_connected(result, cloned_backend.clone());
        let (endpoint, stop_handle) = transport::begin_accept(endpoint, cback).await?;

        self.binds.insert(endpoint.clone(), stop_handle);
        Ok(endpoint)
    }

    async fn connect(&mut self, _endpoint: impl TryIntoEndpoint + 'async_trait) -> ZmqResult<()> {
        unimplemented!()
    }

    fn binds(&self) -> &HashMap<Endpoint, AcceptStopHandle> {
        &self.binds
    }
}

impl RouterSocket {
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
                Some((peer_id, Some(Message::Multipart(messages)))) => {
                    let mut envelope = vec![ZmqMessage {
                        data: peer_id.into(),
                    }];
                    envelope.extend(messages);
                    return Ok(envelope);
                }
                Some((peer_id, None)) => {
                    log::info!("Peer disconnected {:?}", peer_id);
                    self.backend.peers.remove(&peer_id);
                }
                Some((_peer_id, _)) => todo!(),
                None => continue,
            };
        }
    }

    pub async fn send_multipart(&mut self, messages: Vec<ZmqMessage>) -> ZmqResult<()> {
        assert!(messages.len() > 2);
        let peer_id: PeerIdentity = messages[0].data.to_vec().try_into()?;
        match self.backend.peers.get_mut(&peer_id) {
            Some(mut peer) => {
                peer.send_queue
                    .try_send(Message::Multipart(messages[1..].to_vec()))?;
                Ok(())
            }
            None => Err(ZmqError::Other("Destination client not found by identity")),
        }
    }
}

pub struct DealerSocket {
    pub(crate) _inner: FramedIo,
}

impl DealerSocket {
    pub async fn bind(_endpoint: &str) -> ZmqResult<Self> {
        todo!()
    }
}
