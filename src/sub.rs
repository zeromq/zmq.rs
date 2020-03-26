use async_trait::async_trait;
use futures_util::sink::SinkExt;
use tokio::net::TcpStream;
use tokio::stream::StreamExt;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::{Socket, ZmqResult};
use bytes::BytesMut;

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
