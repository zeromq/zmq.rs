use async_trait::async_trait;
use futures_util::sink::SinkExt;
use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::stream::StreamExt;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::{Socket, ZmqResult};

pub struct ReqSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

#[async_trait]
impl Socket for ReqSocket {
    async fn send(&mut self, data: ZmqMessage) -> ZmqResult<()> {
        self._inner.send(Message::Message(data)).await
    }

    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        let message: Option<ZmqResult<Message>> = self._inner.next().await;
        match message {
            Some(Ok(Message::Message(m))) => Ok(m),
            Some(Ok(_)) => Err(ZmqError::OTHER("Wrong message type received")),
            Some(Err(e)) => Err(e),
            None => Err(ZmqError::NO_MESSAGE),
        }
    }
}
