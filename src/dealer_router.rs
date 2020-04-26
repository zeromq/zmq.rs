use async_trait::async_trait;
use futures::select;
use futures_util::sink::SinkExt;
use futures_util::future::FutureExt;
use tokio::net::TcpStream;
use tokio::net::tcp::WriteHalf;
use tokio::net::TcpListener;
use tokio_util::codec::Framed;
use tokio::stream::StreamExt;

use crate::codec::*;
use crate::error::*;
use crate::util::*;
use crate::{Socket, ZmqResult, SocketType};
use bytes::{BytesMut, BufMut, Bytes, Buf};
use std::net::SocketAddr;
use std::sync::Arc;
use futures::lock::Mutex;

pub(crate) struct Peer {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

pub struct RouterSocket {
    pub(crate) peers: Arc<Mutex<Vec<Peer>>>,
    accept_handle: futures::channel::oneshot::Sender<bool>
}

impl RouterSocket {

    async fn peer_connected(socket: tokio::net::TcpStream, peers: Arc<Mutex<Vec<Peer>>>) {

    }

    pub async fn bind(endpoint: &str) -> ZmqResult<Self> {
        let mut listener = tokio::net::TcpListener::bind(endpoint).await?;
        let (sender, receiver) = futures::channel::oneshot::channel::<bool>();
        let mut router_socket = Self { peers: Arc::new(Mutex::new(vec![])), accept_handle: sender };
        let peers = router_socket.peers.clone();
        tokio::spawn(async move {
            let mut stop_callback = receiver.fuse();
            loop {
                select! {
                    incoming = listener.accept().fuse() => {
                        let (socket, _) = incoming.expect("Failed to accept connection");
                        tokio::spawn(RouterSocket::peer_connected(socket, peers.clone()));
                    },
                    _ = stop_callback => {
                        println!("Stop signal received");
                        break
                    }
                }
            }
        });
        Ok(router_socket)
    }
}

#[async_trait]
impl Socket for RouterSocket {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()> {
        unimplemented!()
    }

    async fn recv(&mut self) -> ZmqResult<Vec<u8>> {
        unimplemented!()
    }
}

pub struct DealerSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

impl DealerSocket {
    pub async fn bind(endpoint: &str) -> ZmqResult<Self> {
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