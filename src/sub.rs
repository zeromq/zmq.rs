use async_trait::async_trait;
use futures_util::sink::SinkExt;
use tokio::net::TcpStream;
use tokio::stream::StreamExt;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::util::*;
use crate::{Socket, ZmqResult, SocketType};
use bytes::{BytesMut, BufMut};
use std::net::SocketAddr;

pub struct SubSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

#[async_trait]
impl Socket for SubSocket {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()> {
        Err(ZmqError::SOCKET("This socket doesn't support sending messages"))
    }

    async fn recv(&mut self) -> ZmqResult<Vec<u8>> {
        let message: Option<ZmqResult<Message>> = self._inner.next().await;
        match message {
            Some(Ok(Message::Message(m))) => Ok(m.data.to_vec()),
            Some(Ok(_)) => Err(ZmqError::OTHER("Wrong message type received")),
            Some(Err(e)) => Err(e),
            None => Err(ZmqError::NO_MESSAGE),
        }
    }
}

impl SubSocket {

    pub async fn connect(endpoint: &str) -> ZmqResult<Self> {
        let raw_socket = raw_connect(SocketType::SUB, endpoint).await?;
        Ok(Self { _inner: raw_socket })
    }

    pub async fn subscribe(&mut self, subscription: &str) -> ZmqResult<()> {
        let mut sub = BytesMut::with_capacity(subscription.len() + 1);
        sub.put_u8(1);
        sub.extend_from_slice(subscription.as_bytes());
        self._inner.send(
            Message::Message(ZmqMessage { data: sub.freeze(), more: false })
        ).await?;
        Ok(())
    }

    pub async fn unsubscribe(&mut self, subscription: &str) -> ZmqResult<()> {
        let mut sub = BytesMut::with_capacity(subscription.len() + 1);
        sub.put_u8(0);
        sub.extend_from_slice(subscription.as_bytes());
        self._inner.send(
            Message::Message(ZmqMessage { data: sub.freeze(), more: false })
        ).await?;
        Ok(())
    }
}
