use crate::backend::GenericSocketBackend;
use crate::codec::{Message, ZmqFramedRead};
use crate::fair_queue::FairQueue;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{
    CaptureSocket, Endpoint, MultiPeerBackend, Socket, SocketBackend, SocketEvent, SocketOptions,
    SocketRecv, SocketSend, SocketType, ZmqMessage, ZmqResult,
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
    fn with_options(options: SocketOptions) -> Self {
        let fair_queue = FairQueue::new(options.block_on_no_clients);
        Self {
            backend: Arc::new(GenericSocketBackend::with_options(
                Some(fair_queue.inner()),
                SocketType::DEALER,
                options,
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
impl SocketRecv for DealerSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((_peer_id, Ok(Message::Message(message)))) => {
                    return Ok(message);
                }
                Some((_peer_id, _)) => todo!(),
                None => todo!(),
            };
        }
    }
}

#[async_trait]
impl SocketSend for DealerSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        self.backend
            .send_round_robin(Message::Message(message))
            .await?;
        Ok(())
    }
}

impl CaptureSocket for DealerSocket {}
