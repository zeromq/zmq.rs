use crate::backend::GenericSocketBackend;
use crate::codec::Message;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{Endpoint, MultiPeerBackend, Socket, SocketBackend, SocketType, ZmqMessage, ZmqResult};
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::StreamExt;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::sync::Arc;

pub struct DealerSocket {
    backend: Arc<GenericSocketBackend>,
    fair_queue: mpsc::Receiver<(PeerIdentity, Message)>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for DealerSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl Socket for DealerSocket {
    fn new() -> Self {
        // TODO define buffer size
        let default_queue_size = 100;
        let (queue_sender, fair_queue) = mpsc::channel(default_queue_size);
        Self {
            backend: Arc::new(GenericSocketBackend::new(queue_sender, SocketType::DEALER)),
            fair_queue,
            binds: HashMap::new(),
        }
    }
    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }

    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle, RandomState> {
        &mut self.binds
    }
}

impl DealerSocket {
    pub async fn recv_multipart(&mut self) -> ZmqResult<Vec<ZmqMessage>> {
        loop {
            match self.fair_queue.next().await {
                Some((_peer_id, Message::Multipart(messages))) => {
                    return Ok(messages);
                }
                Some((_peer_id, _)) => todo!(),
                None => todo!(),
            };
        }
    }

    pub async fn send_multipart(&mut self, messages: Vec<ZmqMessage>) -> ZmqResult<()> {
        self.backend
            .send_round_robin(Message::Multipart(messages))
            .await?;
        Ok(())
    }
}
