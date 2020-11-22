use crate::backend::GenericSocketBackend;
use crate::codec::Message;
use crate::transport::AcceptStopHandle;
use crate::{
    transport, util, BlockingSend, Endpoint, Socket, SocketType, TryIntoEndpoint, ZmqError,
    ZmqMessage, ZmqResult,
};
use async_trait::async_trait;
use futures::channel::mpsc;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::sync::Arc;

pub struct PushSocket {
    backend: Arc<GenericSocketBackend>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

#[async_trait]
impl Socket for PushSocket {
    fn new() -> Self {
        // TODO define buffer size
        let default_queue_size = 100;
        let (queue_sender, _fair_queue) = mpsc::channel(default_queue_size);
        Self {
            backend: Arc::new(GenericSocketBackend::new(queue_sender, SocketType::PUSH)),
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

#[async_trait]
impl BlockingSend for PushSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        // In normal scenario this will always be only 1 iteration
        // There can be special case when peer has disconnected and his id is still in
        // RR queue This happens because SegQueue don't have an api to delete
        // items from queue. So in such case we'll just pop item and skip it if
        // we don't have a matching peer in peers map
        let mut message = Message::Message(message);
        loop {
            let next_peer_id = match self.backend.round_robin.pop() {
                Ok(peer) => peer,
                Err(_) => {
                    if let Message::Message(message) = message {
                        return Err(ZmqError::ReturnToSender {
                            reason: "Not connected to peers. Unable to send messages",
                            message,
                        });
                    } else {
                        panic!("Not supposed to happen");
                    }
                }
            };
            match self.backend.peers.get_mut(&next_peer_id) {
                Some(mut peer) => {
                    let send_result = peer.send_queue.try_send(message);
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
                            message = e.into_inner();
                            continue;
                        }
                    };
                }
                None => continue,
            }
        }
    }
}
