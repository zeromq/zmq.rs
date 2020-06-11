use async_trait::async_trait;
use futures_util::sink::SinkExt;
use tokio::net::TcpStream;
use tokio::stream::StreamExt;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::util::raw_connect;
use crate::*;
use crate::{Socket, SocketType, ZmqResult};

/// A ZMQ `REQ` type socket.
pub struct ReqSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

#[async_trait]
impl Socket for ReqSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        let frames = vec![
            "".into(), // delimiter frame
            message,
        ];
        self._inner.send(Message::MultipartMessage(frames)).await
    }

    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        match self._inner.next().await {
            Some(Ok(Message::MultipartMessage(mut message))) => {
                assert!(message.len() == 2);
                assert!(message[0].data.is_empty()); // Ensure that we have delimeter as first part
                Ok(message.pop().unwrap())
            }
            Some(Ok(_)) => Err(ZmqError::Other("Wrong message type received")),
            Some(Err(e)) => Err(e),
            None => Err(ZmqError::NoMessage),
        }
    }
}

impl ReqSocket {
    /// Connect to some endpoint.
    pub async fn connect(endpoint: &str) -> ZmqResult<Self> {
        let raw_socket = raw_connect(SocketType::REQ, endpoint).await?;
        Ok(Self { _inner: raw_socket })
    }
}

pub(crate) struct RepSocketServer {
    pub(crate) _inner: TcpListener,
}

/// A ZMQ `REP` type socket.
pub struct RepSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

#[async_trait]
impl Socket for RepSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        let frames = vec![
            "".into(), // delimiter frame
            message,
        ];
        self._inner.send(Message::MultipartMessage(frames)).await
    }

    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        match self._inner.next().await {
            Some(Ok(Message::MultipartMessage(mut message))) => {
                assert!(message.len() == 2);
                assert!(message[0].data.is_empty()); // Ensure that we have delimeter as first part
                Ok(message.pop().unwrap())
            }
            Some(Ok(_)) => Err(ZmqError::Other("Wrong message type received")),
            Some(Err(e)) => Err(e),
            None => Err(ZmqError::NoMessage),
        }
    }
}

#[async_trait]
impl SocketServer for RepSocketServer {
    async fn accept(&mut self) -> ZmqResult<Box<dyn Socket>> {
        let (socket, _) = self._inner.accept().await?;
        let mut socket = Framed::new(socket, ZmqCodec::new());
        greet_exchange(&mut socket).await?;
        ready_exchange(&mut socket, SocketType::REP).await?;
        Ok(Box::new(RepSocket { _inner: socket }))
    }
}
