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
use bytes::{BytesMut, BufMut, Bytes};
use std::net::SocketAddr;

struct Subscriber {
    pub subscriptions: Vec<Vec<u8>>,
    pub connection: TcpStream,
    //pub sink: FramedWrite<WriteHalf, ZmqCodec>
}

pub struct PubSocket {
    subscribers: Vec<Subscriber>,
    accept_handle: futures::channel::oneshot::Sender<bool>
}

#[async_trait]
impl Socket for PubSocket {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()> {
        let message = ZmqMessage { data: Bytes::from(data), more: false};
        for subscriber in &mut self.subscribers {
            println!("Message sent")
            //subscriber.sink.send(Message::Message(message.clone())).await?
        }
        Ok(())
    }

    async fn recv(&mut self) -> ZmqResult<Vec<u8>> {
        Err(ZmqError::SOCKET("This socket doesn't support receiving messages"))
    }
}

impl PubSocket {
    pub async fn bind(endpoint: &str) -> ZmqResult<Self> {
        let mut listener = TcpListener::bind(endpoint).await?;
        let (sender, receiver) = futures::channel::oneshot::channel::<bool>();
        tokio::spawn(async move {
            loop {
                select! {
                    incoming = listener.accept().fuse() => {
                        let (socket, _) = incoming.expect("Failed to accept connection");
                        let mut socket = Framed::new(socket, ZmqCodec::new());
                        greet_exchange(&mut socket).await.expect("Failed to exchange greetings");
                        ready_exchange(&mut socket, SocketType::PUB).await.expect("Failed to exchange ready");
                        todo!()
                    },
                    stop = receiver.fuse() => {
                        println!("Stop signal received");
                        break
                    }
                }
            }
        });
        Ok(Self { subscribers: vec![], accept_handle: sender })
    }
}


pub struct SubSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

#[async_trait]
impl Socket for SubSocket {
    async fn send(&mut self, _data: Vec<u8>) -> ZmqResult<()> {
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
