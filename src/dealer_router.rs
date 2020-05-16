use async_trait::async_trait;
use futures::channel::mpsc::*;
use futures::stream::StreamExt;
use futures::{select, Future, SinkExt};
use futures_util::future::FutureExt;
use tokio::net::TcpStream;
use tokio::stream::Stream;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::util;
use crate::util::*;
use crate::{Socket, SocketType, ZmqResult};
use dashmap::DashMap;
use std::sync::Arc;

pub struct RouterSocket {
    pub(crate) peers: Arc<DashMap<PeerIdentity, Peer>>,
    _accept_close_handle: futures::channel::oneshot::Sender<bool>,
}

impl Drop for RouterSocket {
    fn drop(&mut self) {
        self.peers.clear()
    }
}

impl RouterSocket {
    pub async fn bind(endpoint: &str) -> ZmqResult<Self> {
        let peers = Arc::new(DashMap::new());
        let router_socket = Self {
            peers: peers.clone(),
            _accept_close_handle: util::start_accepting_connections(
                endpoint,
                peers,
                SocketType::ROUTER,
            )
            .await?,
        };
        Ok(router_socket)
    }

    pub async fn recv_multipart(&mut self) -> ZmqResult<Vec<ZmqMessage>> {
        for mut peer in self.peers.iter_mut() {
            match peer.value_mut().recv_queue.try_next() {
                Ok(Some(Message::MultipartMessage(messages))) => return Ok(messages),
                Err(_) => continue,
                _ => todo!(),
            }
        }
        Err(ZmqError::NoMessage)
    }

    pub async fn send_multipart(&mut self, messages: Vec<ZmqMessage>) -> ZmqResult<()> {
        assert!(messages.len() > 2);
        let peer_id: PeerIdentity = messages[0].data.to_vec().into();
        match self.peers.get_mut(&peer_id) {
            Some(mut peer) => {
                peer.send_queue
                    .try_send(Message::MultipartMessage(messages[1..].to_vec()))?;
                Ok(())
            }
            None => return Err(ZmqError::Other("Destination client not found by identity")),
        }
    }
}

#[async_trait]
impl Socket for RouterSocket {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()> {
        unimplemented!()
    }

    async fn recv(&mut self) -> ZmqResult<Vec<u8>> {
        for mut peer in self.peers.iter_mut() {
            match peer.value_mut().recv_queue.try_next() {
                Ok(Some(Message::MultipartMessage(messages))) => {
                    let mut data = Vec::new();
                    for m in messages {
                        data.extend(m.data.to_vec());
                    }
                    return Ok(data);
                }
                Err(_) => continue,
                _ => todo!(),
            }
        }
        Err(ZmqError::NoMessage)
    }
}

pub struct DealerSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

impl DealerSocket {
    pub async fn bind(_endpoint: &str) -> ZmqResult<Self> {
        todo!()
    }
}

#[async_trait]
impl Socket for DealerSocket {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()> {
        unimplemented!()
    }

    async fn recv(&mut self) -> ZmqResult<Vec<u8>> {
        unimplemented!()
    }
}
