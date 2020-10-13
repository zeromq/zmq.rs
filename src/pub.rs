use crate::codec::*;
use crate::endpoint::{Endpoint, TryIntoEndpoint};
use crate::message::*;
use crate::transport::{self, AcceptStopChannel};
use crate::util::*;
use crate::{util, MultiPeer, NonBlockingSend, Socket, SocketBackend, SocketType, ZmqResult};

use async_trait::async_trait;
use dashmap::DashMap;
use futures::channel::{mpsc, oneshot};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct Subscriber {
    pub(crate) subscriptions: Vec<Vec<u8>>,
    pub(crate) send_queue: mpsc::Sender<Message>,
    pub(crate) _io_close_handle: futures::channel::oneshot::Sender<bool>,
}

pub(crate) struct PubSocketBackend {
    subscribers: DashMap<PeerIdentity, Subscriber>,
}

#[async_trait]
impl SocketBackend for PubSocketBackend {
    async fn message_received(&self, peer_id: &PeerIdentity, message: Message) {
        let message = match message {
            Message::Message(m) => m,
            _ => return,
        };
        let data: Vec<u8> = message.into();
        if data.is_empty() {
            return;
        }
        match data[0] {
            1 => {
                // Subscribe
                self.subscribers
                    .get_mut(&peer_id)
                    .unwrap()
                    .subscriptions
                    .push(Vec::from(&data[1..]));
            }
            0 => {
                // Unsubscribe
                let mut del_index = None;
                let sub = Vec::from(&data[1..]);
                for (idx, subscription) in self
                    .subscribers
                    .get(&peer_id)
                    .unwrap()
                    .subscriptions
                    .iter()
                    .enumerate()
                {
                    if &sub == subscription {
                        del_index = Some(idx);
                        break;
                    }
                }
                if let Some(index) = del_index {
                    self.subscribers
                        .get_mut(&peer_id)
                        .unwrap()
                        .subscriptions
                        .remove(index);
                }
            }
            _ => (),
        }
    }

    fn socket_type(&self) -> SocketType {
        SocketType::PUB
    }

    fn shutdown(&self) {
        self.subscribers.clear();
    }
}

#[async_trait]
impl MultiPeer for PubSocketBackend {
    async fn peer_connected(
        &self,
        peer_id: &PeerIdentity,
    ) -> (mpsc::Receiver<Message>, oneshot::Receiver<bool>) {
        let default_queue_size = 100;
        let (out_queue, out_queue_receiver) = mpsc::channel(default_queue_size);
        let (stop_handle, stop_callback) = oneshot::channel::<bool>();

        self.subscribers.insert(
            peer_id.clone(),
            Subscriber {
                subscriptions: vec![],
                send_queue: out_queue,
                _io_close_handle: stop_handle,
            },
        );
        (out_queue_receiver, stop_callback)
    }

    async fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        log::info!("Client disconnected {:?}", peer_id);
        self.subscribers.remove(peer_id);
    }
}

pub struct PubSocket {
    pub(crate) backend: Arc<PubSocketBackend>,
    binds: HashMap<Endpoint, AcceptStopChannel>,
}

impl Drop for PubSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

impl NonBlockingSend for PubSocket {
    fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        for mut subscriber in self.backend.subscribers.iter_mut() {
            for sub_filter in &subscriber.subscriptions {
                if sub_filter.as_slice() == &message.data[0..sub_filter.len()] {
                    let _res = subscriber
                        .send_queue
                        .try_send(Message::Message(message.clone()));
                    // TODO handle result
                    break;
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Socket for PubSocket {
    fn new() -> Self {
        Self {
            backend: Arc::new(PubSocketBackend {
                subscribers: DashMap::new(),
            }),
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

    async fn connect(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) -> ZmqResult<()> {
        let endpoint = endpoint.try_into()?;

        let connect_result = transport::connect(endpoint).await;
        util::peer_connected(connect_result, self.backend.clone()).await;
        Ok(())
    }

    fn binds(&self) -> &HashMap<Endpoint, AcceptStopChannel> {
        &self.binds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::tests::{
        test_bind_to_any_port_helper, test_bind_to_unspecified_interface_helper,
    };
    use crate::ZmqResult;
    use std::net::IpAddr;

    #[tokio::test]
    async fn test_bind_to_any_port() -> ZmqResult<()> {
        let s = PubSocket::new();
        test_bind_to_any_port_helper(s).await
    }

    #[tokio::test]
    async fn test_bind_to_any_ipv4_interface() -> ZmqResult<()> {
        let any_ipv4: IpAddr = "0.0.0.0".parse().unwrap();
        let s = PubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv4, s, 4000).await
    }

    #[tokio::test]
    async fn test_bind_to_any_ipv6_interface() -> ZmqResult<()> {
        let any_ipv6: IpAddr = "::".parse().unwrap();
        let s = PubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv6, s, 4010).await
    }
}
