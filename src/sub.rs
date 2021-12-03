use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::ZmqResult;
use crate::message::*;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{
    MultiPeerBackend, Socket, SocketBackend, SocketEvent, SocketOptions, SocketRecv, SocketType
};

use crate::backend::Peer;
use dashmap::DashMap;
use parking_lot::Mutex;
use crate::fair_queue::QueueInner;
use crate::fair_queue::FairQueue;
use crossbeam::queue::SegQueue;
use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub(crate) struct SubSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, Peer>,
    fair_queue_inner: Option<Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>>,
    pub(crate) round_robin: SegQueue<PeerIdentity>,
    socket_type: SocketType,
    socket_options: SocketOptions,
    pub(crate) socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
}

impl SubSocketBackend {
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
        }
    }
}

impl SocketBackend for SubSocketBackend {
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

impl MultiPeerBackend for SubSocketBackend {
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

pub struct SubSocket {
    backend: Arc<SubSocketBackend>,
    fair_queue: FairQueue<ZmqFramedRead, PeerIdentity>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
    subs: HashSet<String>,
}

impl Drop for SubSocket {
    fn drop(&mut self) {
        self.backend.shutdown()
    }
}

impl SubSocket {
    pub async fn subscribe(&mut self, subscription: &str) -> ZmqResult<()> {
        let mut buf = BytesMut::with_capacity(subscription.len() + 1);
        buf.put_u8(1);
        buf.extend_from_slice(subscription.as_bytes());
        // let message = format!("\0x1{}", subscription);
        let message: ZmqMessage = ZmqMessage::from(buf.freeze());
        for mut peer in self.backend.peers.iter_mut() {
            peer.send_queue
                .send(Message::Message(message.clone()))
                .await?;
        }
	self.subs.insert(subscription.to_string());
        Ok(())
    }

    pub async fn unsubscribe(&mut self, subscription: &str) -> ZmqResult<()> {
	self.subs.insert(subscription.to_string());
        let mut buf = BytesMut::with_capacity(subscription.len() + 1);
        buf.put_u8(0);
        buf.extend_from_slice(subscription.as_bytes());
        let message = ZmqMessage::from(buf.freeze());
        for mut peer in self.backend.peers.iter_mut() {
            peer.send_queue
                .send(Message::Message(message.clone()))
                .await?;
        }
        Ok(())
    }
}


#[async_trait]
impl Socket for SubSocket {
    fn with_options(options: SocketOptions) -> Self {
        let fair_queue = FairQueue::new(true);
        Self {
            backend: Arc::new(SubSocketBackend::with_options(
                Some(fair_queue.inner()),
                SocketType::SUB,
                options,
            )),
            fair_queue,
            binds: HashMap::new(),
	    subs: HashSet::new(),
        }
    }

    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }

    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle> {
        &mut self.binds
    }

    fn monitor(&mut self) -> mpsc::Receiver<SocketEvent> {
        let (sender, receiver) = mpsc::channel(1024);
        self.backend.socket_monitor.lock().replace(sender);
        receiver
    }
}

#[async_trait]
impl SocketRecv for SubSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((_peer_id, Ok(Message::Message(message)))) => {
                    return Ok(message);
                }
                Some((_peer_id, _)) => todo!(),
                None => todo!(),
            }
        }
    }
}
