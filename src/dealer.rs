use crate::backend::GenericSocketBackend;
use crate::codec::Message;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{
    transport, util, Endpoint, Socket, SocketBackend, SocketType, TryIntoEndpoint, ZmqError,
    ZmqMessage, ZmqResult,
};
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

    async fn bind(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) -> ZmqResult<Endpoint> {
        let endpoint = endpoint.try_into()?;

        let cloned_backend = self.backend.clone();
        let cback = move |result| util::peer_connected(result, cloned_backend.clone());
        let (endpoint, stop_handle) = transport::begin_accept(endpoint, cback).await?;

        self.binds.insert(endpoint.clone(), stop_handle);
        Ok(endpoint)
    }

    fn binds(&self) -> &HashMap<Endpoint, AcceptStopHandle, RandomState> {
        &self.binds
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
        // In normal scenario this will always be only 1 iteration
        // There can be special case when peer has disconnected and his id is still in
        // RR queue This happens because SegQueue don't have an api to delete
        // items from queue. So in such case we'll just pop item and skip it if
        // we don't have a matching peer in peers map
        let mut messages = Message::Multipart(messages);
        loop {
            let next_peer_id = match self.backend.round_robin.pop() {
                Ok(peer) => peer,
                Err(_) => {
                    if let Message::Multipart(messages) = messages {
                        return Err(ZmqError::ReturnToSenderMultipart {
                            reason: "Not connected to peers. Unable to send messages",
                            messages,
                        });
                    } else {
                        panic!("Not supposed to happen");
                    }
                }
            };
            match self.backend.peers.get_mut(&next_peer_id) {
                Some(mut peer) => {
                    let send_result = peer.send_queue.try_send(messages);
                    match send_result {
                        Ok(()) => {
                            self.backend.round_robin.push(next_peer_id.clone());
                            return Ok(());
                        }
                        Err(e) => {
                            if e.is_full() {
                                // Try again later
                                self.backend.round_robin.push(next_peer_id.clone());
                            }
                            messages = e.into_inner();
                            continue;
                        }
                    };
                }
                None => continue,
            }
        }
    }
}
