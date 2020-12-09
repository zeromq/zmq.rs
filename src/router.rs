use async_trait::async_trait;
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::convert::TryInto;
use std::sync::Arc;

use crate::backend::GenericSocketBackend;
use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::{ZmqError, ZmqResult};
use crate::fair_queue::FairQueue;
use crate::message::*;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{MultiPeerBackend, SocketEvent, SocketType};
use crate::{Socket, SocketBackend};
use futures::channel::mpsc;
use futures::SinkExt;

pub struct RouterSocket {
    backend: Arc<GenericSocketBackend>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
    fair_queue: FairQueue<ZmqFramedRead, PeerIdentity>,
}

impl Drop for RouterSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl Socket for RouterSocket {
    fn new() -> Self {
        let fair_queue = FairQueue::new(true);
        Self {
            backend: Arc::new(GenericSocketBackend::new(
                Some(fair_queue.inner()),
                SocketType::ROUTER,
            )),
            binds: HashMap::new(),
            fair_queue,
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

impl RouterSocket {
    pub async fn recv_multipart(&mut self) -> ZmqResult<Vec<ZmqMessage>> {
        loop {
            match self.fair_queue.next().await {
                Some((peer_id, Ok(Message::Multipart(mut messages)))) => {
                    messages.insert(0, peer_id.into());
                    return Ok(messages);
                }
                Some((_peer_id, _)) => todo!(),
                None => todo!(),
            };
        }
    }

    pub async fn send_multipart(&mut self, messages: Vec<ZmqMessage>) -> ZmqResult<()> {
        let peer_id: PeerIdentity = messages[0].data.to_vec().try_into()?;
        match self.backend.peers.get_mut(&peer_id) {
            Some(mut peer) => {
                peer.send_queue
                    .send(Message::Multipart(messages[1..].to_vec()))
                    .await?;
                Ok(())
            }
            None => Err(ZmqError::Other("Destination client not found by identity")),
        }
    }
}
