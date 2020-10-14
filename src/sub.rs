use crate::codec::*;
use crate::endpoint::{Endpoint, TryIntoEndpoint};
use crate::error::{ZmqError, ZmqResult};
use crate::fair_queue::FairQueue;
use crate::message::*;
use crate::transport::{self, AcceptStopHandle};
use crate::util::{self, FairQueueProcessor, PeerIdentity};
use crate::{BlockingRecv, MultiPeer, Socket, SocketBackend, SocketType};

use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use dashmap::DashMap;
use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct SubPeer {
    pub(crate) _identity: PeerIdentity,
    pub(crate) send_queue: mpsc::Sender<Message>,
    pub(crate) recv_queue_in: mpsc::Sender<Message>,
    pub(crate) _io_close_handle: futures::channel::oneshot::Sender<bool>,
}

pub(crate) struct SubSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, SubPeer>,
    pub(crate) peer_queue_in: mpsc::Sender<(PeerIdentity, mpsc::Receiver<Message>)>,
}

pub struct SubSocket {
    backend: Arc<SubSocketBackend>,
    fair_queue: mpsc::Receiver<(PeerIdentity, Message)>,
    _fair_queue_close_handle: oneshot::Sender<bool>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for SubSocket {
    fn drop(&mut self) {
        self.backend.shutdown()
    }
}

#[async_trait]
impl SocketBackend for SubSocketBackend {
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
        SocketType::SUB
    }

    fn shutdown(&self) {
        self.peers.clear();
    }
}

#[async_trait]
impl MultiPeer for SubSocketBackend {
    async fn peer_connected(
        &self,
        peer_id: &PeerIdentity,
    ) -> (mpsc::Receiver<Message>, oneshot::Receiver<bool>) {
        let default_queue_size = 100;
        let (out_queue, out_queue_receiver) = mpsc::channel(1);
        let (in_queue, in_queue_receiver) = mpsc::channel::<Message>(default_queue_size);
        let (stop_handle, stop_callback) = oneshot::channel::<bool>();

        self.peers.insert(
            peer_id.clone(),
            SubPeer {
                _identity: peer_id.clone(),
                send_queue: out_queue,
                recv_queue_in: in_queue,
                _io_close_handle: stop_handle,
            },
        );
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

impl SubSocket {
    pub async fn subscribe(&mut self, subscription: &str) -> ZmqResult<()> {
        let mut message = BytesMut::with_capacity(subscription.len() + 1);
        message.put_u8(1);
        message.extend_from_slice(subscription.as_bytes());
        // let message = format!("\0x1{}", subscription);
        for mut peer in self.backend.peers.iter_mut() {
            peer.send_queue
                .send(Message::Message(message.clone().into()))
                .await?;
        }
        Ok(())
    }

    pub async fn unsubscribe(&mut self, subscription: &str) -> ZmqResult<()> {
        let mut message = BytesMut::with_capacity(subscription.len() + 1);
        message.put_u8(0);
        message.extend_from_slice(subscription.as_bytes());
        for mut peer in self.backend.peers.iter_mut() {
            peer.send_queue
                .send(Message::Message(message.clone().into()))
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl Socket for SubSocket {
    fn new() -> Self {
        // TODO define buffer size
        let default_queue_size = 100;
        let (queue_sender, fair_queue) = mpsc::channel(default_queue_size);
        let (peer_in, peer_out) = mpsc::channel(default_queue_size);
        let (fair_queue_close_handle, fqueue_close_recevier) = oneshot::channel();
        tokio::spawn(util::process_fair_queue_messages(FairQueueProcessor {
            fair_queue_stream: FairQueue::new(),
            socket_incoming_queue: queue_sender,
            peer_queue_in: peer_out,
            _io_close_handle: fqueue_close_recevier,
        }));
        Self {
            backend: Arc::new(SubSocketBackend {
                peers: Default::default(),
                peer_queue_in: peer_in,
            }),
            fair_queue,
            _fair_queue_close_handle: fair_queue_close_handle,
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

    async fn unbind(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) -> ZmqResult<()> {
        let endpoint = endpoint.try_into()?;

        let stop_handle = self.binds.remove(&endpoint);
        let stop_handle = stop_handle.ok_or(ZmqError::NoSuchBind(endpoint))?;
        stop_handle.0.shutdown().await
    }

    async fn connect(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) -> ZmqResult<()> {
        let endpoint = endpoint.try_into()?;

        let connect_result = transport::connect(endpoint).await;
        util::peer_connected(connect_result, self.backend.clone()).await;
        Ok(())
    }

    fn binds(&self) -> &HashMap<Endpoint, AcceptStopHandle> {
        &self.binds
    }
}

#[async_trait]
impl BlockingRecv for SubSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((_peer_id, Message::Message(message))) => {
                    return Ok(message);
                }
                Some((_peer_id, _)) => todo!(),
                None => todo!(),
            }
        }
    }
}
