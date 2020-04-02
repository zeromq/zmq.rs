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
use std::sync::{Arc, Mutex};

pub(crate) struct Subscriber {
    pub subscriptions: Vec<Vec<u8>>,
    pub connection: tokio_util::codec::FramedWrite<tokio::io::WriteHalf<tokio::net::TcpStream>, ZmqCodec>,
}

pub struct PubSocket {
    pub(crate) subscribers: Arc<Mutex<Vec<Subscriber>>>,
    accept_handle: futures::channel::oneshot::Sender<bool>
}

#[async_trait]
impl Socket for PubSocket {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()> {
        let message = ZmqMessage { data: Bytes::from(data), more: false};
        let mut subscribers = self.subscribers.lock().expect("Failed to lock subscribers");
        for subscriber in subscribers.iter_mut() {
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
    async fn handle_subscriber(socket: tokio::net::TcpStream, subscribers: Arc<Mutex<Vec<Subscriber>>>) {
        let (read, write) = tokio::io::split(socket);
        let mut read_part = tokio_util::codec::FramedRead::new(read, ZmqCodec::new());
        let mut write_part = tokio_util::codec::FramedWrite::new(write, ZmqCodec::new());
        //let mut socket = Framed::new(socket, ZmqCodec::new());
        //greet_exchange(&mut socket).await.expect("Failed to exchange greetings");
        {
            write_part
                .send(Message::Greeting(ZmqGreeting::default()))
                .await;

            let greeting: Option<Result<Message, ZmqError>> = read_part.next().await;

            match greeting {
                Some(Ok(Message::Greeting(greet))) => match greet.version {
                    (3, 0) => Ok(()),
                    _ => Err(ZmqError::OTHER("Unsupported protocol version")),
                },
                _ => Err(ZmqError::CODEC("Failed Greeting exchange")),
            }.unwrap();
        }
        {
            write_part.send(Message::Command(ZmqCommand::ready(SocketType::PUB))).await;

            let _ready_repl: Option<ZmqResult<Message>> = read_part.next().await;
        }

        let mut subscriber_id = None;
        {
            let mut locked_subscribers = subscribers.lock().expect("Failed to lock subscribers");
            locked_subscribers.push(Subscriber { subscriptions: vec![], connection: write_part});
            subscriber_id = Some(locked_subscribers.len());
        }

        loop {
            let msg: Option<ZmqResult<Message>> = read_part.next().await;
            match msg {
                Some(Ok(Message::Message(message))) => {
                    let data = message.data.to_vec();
                    if data.len() < 1 {
                        panic!("Unable to handle message")
                    }

                },
                _ => {
                    todo!()
                }
            }
        }
    }

    pub async fn bind(endpoint: &str) -> ZmqResult<Self> {
        let mut listener = tokio::net::TcpListener::bind(endpoint).await?;
        let (sender, receiver) = futures::channel::oneshot::channel::<bool>();
        let mut pub_socket = Self { subscribers: Arc::new(Mutex::new(vec![])), accept_handle: sender };
        let subscribers = pub_socket.subscribers.clone();
        tokio::spawn(async move {
            let mut stop_callback = receiver.fuse();
            loop {
                select! {
                    incoming = listener.accept().fuse() => {
                        let (socket, _) = incoming.expect("Failed to accept connection");
                        tokio::spawn(PubSocket::handle_subscriber(socket, subscribers.clone()));
                    },
                    _ = stop_callback => {
                        println!("Stop signal received");
                        break
                    }
                }
            }
        });
        Ok(pub_socket)
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
