use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::ZmqResult;
use crate::message::*;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{BlockingSend, MultiPeerBackend, Socket, SocketBackend, SocketType};
use futures::channel::oneshot;

use async_trait::async_trait;
use dashmap::DashMap;
use futures::SinkExt;
use futures_codec::FramedWrite;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Arc;

pub(crate) struct Subscriber {
    pub(crate) subscriptions: Vec<Vec<u8>>,
    pub(crate) send_queue: FramedWrite<Box<dyn FrameableWrite>, ZmqCodec>,
    _subscription_coro_stop: oneshot::Sender<()>,
}

pub(crate) struct PubSocketBackend {
    subscribers: DashMap<PeerIdentity, Subscriber>,
}

impl PubSocketBackend {
    fn message_received(&self, peer_id: &PeerIdentity, message: Message) {
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
}

impl SocketBackend for PubSocketBackend {
    fn socket_type(&self) -> SocketType {
        SocketType::PUB
    }

    fn shutdown(&self) {
        self.subscribers.clear();
    }
}

impl MultiPeerBackend for PubSocketBackend {
    fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        let (mut recv_queue, send_queue) = io.into_parts();
        // TODO provide handling for recv_queue
        let (sender, mut stop_receiver) = oneshot::channel();
        self.subscribers.insert(
            peer_id.clone(),
            Subscriber {
                subscriptions: vec![],
                send_queue,
                _subscription_coro_stop: sender,
            },
        );
        let backend = self;
        let peer_id = peer_id.clone();
        tokio::spawn(async move {
            use futures::StreamExt;
            loop {
                tokio::select! {
                     _ = &mut stop_receiver => {
                         break;
                     },
                     message = &mut recv_queue.next() => {
                        match message {
                            Some(Ok(m)) => backend.message_received(&peer_id, m),
                            Some(Err(e)) => {
                                dbg!(e);
                                backend.peer_disconnected(&peer_id);
                                break;
                            }
                            None => {
                                backend.peer_disconnected(&peer_id);
                                break
                            }
                        }

                     }
                }
            }
        });
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        log::info!("Client disconnected {:?}", peer_id);
        self.subscribers.remove(peer_id);
    }
}

pub struct PubSocket {
    pub(crate) backend: Arc<PubSocketBackend>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for PubSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl BlockingSend for PubSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        let mut dead_peers = Vec::new();
        for mut subscriber in self.backend.subscribers.iter_mut() {
            for sub_filter in &subscriber.subscriptions {
                if sub_filter.as_slice() == &message.data[0..sub_filter.len()] {
                    // TODO should use try_send here
                    let res = subscriber
                        .send_queue
                        .send(Message::Message(message.clone()))
                        .await;
                    match res {
                        Ok(()) => {}
                        Err(CodecError::Io(e)) => {
                            if e.kind() == ErrorKind::BrokenPipe {
                                dead_peers.push(subscriber.key().clone());
                            } else {
                                dbg!(e);
                            }
                        }
                        Err(e) => {
                            dbg!(e);
                            todo!()
                        }
                    }
                    break;
                }
            }
        }
        for peer in dead_peers {
            self.backend.peer_disconnected(&peer);
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

    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }

    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle> {
        &mut self.binds
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
