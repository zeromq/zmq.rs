use crate::backend::GenericSocketBackend;
use crate::codec::{Message, ZmqFramedRead};
use crate::fair_queue::FairQueue;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{
    BlockingRecv, BlockingSend, Endpoint, MultiPeerBackend, Socket, SocketBackend, SocketEvent,
    SocketType, ZmqResult,
};
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::StreamExt;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::sync::Arc;

pub struct DealerSocket {
    backend: Arc<GenericSocketBackend>,
    fair_queue: FairQueue<ZmqFramedRead, PeerIdentity>,
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
        let fair_queue = FairQueue::new(true);
        Self {
            backend: Arc::new(GenericSocketBackend::new(
                Some(fair_queue.inner()),
                SocketType::DEALER,
            )),
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

    fn monitor(&mut self) -> mpsc::Receiver<SocketEvent> {
        let (sender, receiver) = mpsc::channel(1024);
        self.backend.socket_monitor.lock().replace(sender);
        receiver
    }
}

#[async_trait]
impl BlockingSend for DealerSocket {
    async fn send(&mut self, message: Message) -> ZmqResult<()> {
        self.backend.send_round_robin(message).await?;
        Ok(())
    }
}

#[async_trait]
impl BlockingRecv for DealerSocket {
    async fn recv(&mut self) -> ZmqResult<Message> {
        loop {
            match self.fair_queue.next().await {
                Some((_peer_id, Ok(m))) => {
                    return Ok(m);
                }
                Some((_peer_id, _)) => todo!(),
                None => todo!(),
            };
        }
    }
}
