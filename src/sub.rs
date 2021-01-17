use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::ZmqResult;
use crate::message::*;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{MultiPeerBackend, Socket, SocketBackend, SocketEvent, SocketRecv, SocketType};

use crate::backend::GenericSocketBackend;
use crate::fair_queue::FairQueue;
use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;

pub struct SubSocket {
    backend: Arc<GenericSocketBackend>,
    fair_queue: FairQueue<ZmqFramedRead, PeerIdentity>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
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
        Ok(())
    }

    pub async fn unsubscribe(&mut self, subscription: &str) -> ZmqResult<()> {
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
    fn new() -> Self {
        let fair_queue = FairQueue::new(true);
        Self {
            backend: Arc::new(GenericSocketBackend::new(
                Some(fair_queue.inner()),
                SocketType::SUB,
            )),
            fair_queue,
            binds: HashMap::new(),
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
