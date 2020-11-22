use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::ZmqResult;
use crate::message::*;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{BlockingRecv, MultiPeerBackend, Socket, SocketBackend, SocketType};

use crate::backend::GenericSocketBackend;
use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;

pub struct SubSocket {
    backend: Arc<GenericSocketBackend>,
    fair_queue: mpsc::Receiver<(PeerIdentity, Message)>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for SubSocket {
    fn drop(&mut self) {
        self.backend.shutdown()
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
        Self {
            backend: Arc::new(GenericSocketBackend::new(queue_sender, SocketType::SUB)),
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
