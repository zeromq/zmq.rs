use async_trait::async_trait;
use futures_util::sink::SinkExt;
use tokio::net::TcpStream;
use tokio::stream::StreamExt;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::{Socket, ZmqResult, SocketType};
use bytes::BytesMut;
use crate::util::raw_connect;

pub struct ReqSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

#[async_trait]
impl Socket for ReqSocket {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()> {
        let mut f_data = BytesMut::new();
        f_data.extend_from_slice(data.as_ref());
        let frames = vec![
            ZmqMessage {
                data: BytesMut::new().freeze(),
                more: true,
            }, // delimiter frame
            ZmqMessage {
                data: f_data.freeze(),
                more: false,
            },
        ];
        self._inner.send(Message::MultipartMessage(frames)).await
    }

    async fn recv(&mut self) -> ZmqResult<Vec<u8>> {
        {
            let delimeter: Option<ZmqResult<Message>> = self._inner.next().await;
            let delim = match delimeter {
                Some(Ok(Message::Message(m))) => m,
                Some(Ok(_)) => return Err(ZmqError::OTHER("Wrong message type received")),
                Some(Err(e)) => return Err(e),
                None => return Err(ZmqError::NO_MESSAGE),
            };
            assert!(delim.data.is_empty() && delim.more); // Drop delimeter frame
        }
        let message: Option<ZmqResult<Message>> = self._inner.next().await;
        match message {
            Some(Ok(Message::Message(m))) => Ok(m.data.to_vec()),
            Some(Ok(_)) => Err(ZmqError::OTHER("Wrong message type received")),
            Some(Err(e)) => Err(e),
            None => Err(ZmqError::NO_MESSAGE),
        }
    }
}

impl ReqSocket {
    pub async fn connect(endpoint: &str) -> ZmqResult<Self> {
        let raw_socket = raw_connect(SocketType::REQ, endpoint).await?;
        Ok(Self { _inner: raw_socket })
    }
}