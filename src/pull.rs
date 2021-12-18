use crate::backend::GenericSocketBackend;
use crate::codec::{Message, ZmqFramedRead};
use crate::fair_queue::FairQueue;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{
    Endpoint, MultiPeerBackend, Socket, SocketEvent, SocketOptions, SocketRecv, SocketType,
    ZmqMessage, ZmqResult,
};
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::StreamExt;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::sync::Arc;

pub struct PullSocket {
    backend: Arc<GenericSocketBackend>,
    fair_queue: FairQueue<ZmqFramedRead, PeerIdentity>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

#[async_trait]
impl Socket for PullSocket {
    fn with_options(options: SocketOptions) -> Self {
        let fair_queue = FairQueue::new(true);
        Self {
            backend: Arc::new(GenericSocketBackend::with_options(
                Some(fair_queue.inner()),
                SocketType::PULL,
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
impl SocketRecv for PullSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((_peer_id, Ok(Message::Message(message)))) => {
                    return Ok(message);
                }
                Some((_peer_id, Ok(msg))) => todo!("Unimplemented message: {:?}", msg),
                Some((peer_id, Err(_))) => {
                    self.backend.peer_disconnected(&peer_id);
                }
                None => todo!(),
            };
        }
    }
}
