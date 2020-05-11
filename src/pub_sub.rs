use async_trait::async_trait;
use futures::select;
use futures_util::future::FutureExt;
use futures_util::sink::SinkExt;
use tokio::net::TcpStream;
use tokio::stream::StreamExt;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::util::*;
use crate::{Socket, SocketType, ZmqResult};
use bytes::{BufMut, Bytes, BytesMut};
use futures::lock::Mutex;
use std::sync::Arc;

pub(crate) struct Subscriber {
    pub subscriptions: Vec<Vec<u8>>,
    pub connection:
        tokio_util::codec::FramedWrite<tokio::io::WriteHalf<tokio::net::TcpStream>, ZmqCodec>,
}

pub struct PubSocket {
    pub(crate) subscribers: Arc<Mutex<Vec<Subscriber>>>,
    _accept_close_handle: futures::channel::oneshot::Sender<bool>,
}

#[async_trait]
impl Socket for PubSocket {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()> {
        let message = ZmqMessage {
            data: Bytes::from(data),
        };
        let mut fanout = vec![];
        {
            let mut subscribers = self.subscribers.lock().await;
            for subscriber in subscribers.iter_mut() {
                for sub_filter in &subscriber.subscriptions {
                    if sub_filter.as_slice() == &message.data[0..sub_filter.len()] {
                        fanout.push(
                            subscriber
                                .connection
                                .send(Message::Message(message.clone())),
                        );
                        break;
                    }
                }
            }
            println!("Sending message to all piers");
            futures::future::join_all(fanout).await; // Ignore errors for now
        }
        Ok(())
    }

    async fn recv(&mut self) -> ZmqResult<Vec<u8>> {
        Err(ZmqError::Socket(
            "This socket doesn't support receiving messages",
        ))
    }
}

impl PubSocket {
    async fn handle_subscriber(
        socket: tokio::net::TcpStream,
        subscribers: Arc<Mutex<Vec<Subscriber>>>,
    ) {
        let (read, write) = tokio::io::split(socket);
        let mut read_part = tokio_util::codec::FramedRead::new(read, ZmqCodec::new());
        let mut write_part = tokio_util::codec::FramedWrite::new(write, ZmqCodec::new());

        greet_exchange_w_parts(&mut write_part, &mut read_part)
            .await
            .expect("Failed to exchange greetings");

        ready_exchange_w_parts(&mut write_part, &mut read_part, SocketType::PUB)
            .await
            .expect("Failed to exchange ready messages");

        let mut subscriber_id = None;
        {
            let mut locked_subscribers = subscribers.lock().await;
            locked_subscribers.push(Subscriber {
                subscriptions: vec![],
                connection: write_part,
            });
            subscriber_id = Some(locked_subscribers.len() - 1);
        }

        loop {
            let msg: Option<ZmqResult<Message>> = read_part.next().await;
            match msg {
                Some(Ok(Message::Message(message))) => {
                    let data = message.data.to_vec();
                    if data.len() < 1 {
                        panic!("Unable to handle message")
                    }
                    {
                        let mut locked_subscribers = subscribers.lock().await;
                        match data[0] {
                            1 => {
                                // Subscribe
                                locked_subscribers[subscriber_id.unwrap()]
                                    .subscriptions
                                    .push(Vec::from(&data[1..]));
                            }
                            0 => {
                                // Unsubscribe
                                let mut del_index = None;
                                let sub = Vec::from(&data[1..]);
                                for (idx, subscription) in locked_subscribers
                                    [subscriber_id.unwrap()]
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
                                    locked_subscribers[subscriber_id.unwrap()]
                                        .subscriptions
                                        .remove(index);
                                }
                            }
                            _ => panic!("Malformed message"),
                        }
                    }
                }
                Some(other) => {
                    dbg!(other);
                    todo!()
                }
                None => {
                    println!("None received");
                    // TODO implement handling for disconnected client
                    todo!()
                }
            }
        }
    }

    pub async fn bind(endpoint: &str) -> ZmqResult<Self> {
        let mut listener = tokio::net::TcpListener::bind(endpoint).await?;
        let (sender, receiver) = futures::channel::oneshot::channel::<bool>();
        let pub_socket = Self {
            subscribers: Arc::new(Mutex::new(vec![])),
            _accept_close_handle: sender,
        };
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
        Err(ZmqError::Socket(
            "This socket doesn't support sending messages",
        ))
    }

    async fn recv(&mut self) -> ZmqResult<Vec<u8>> {
        let message: Option<ZmqResult<Message>> = self._inner.next().await;
        match message {
            Some(Ok(Message::Message(m))) => Ok(m.data.to_vec()),
            Some(Ok(_)) => Err(ZmqError::Other("Wrong message type received")),
            Some(Err(e)) => Err(e),
            None => Err(ZmqError::NoMessage),
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
        self._inner
            .send(Message::Message(ZmqMessage { data: sub.freeze() }))
            .await?;
        Ok(())
    }

    pub async fn unsubscribe(&mut self, subscription: &str) -> ZmqResult<()> {
        let mut sub = BytesMut::with_capacity(subscription.len() + 1);
        sub.put_u8(0);
        sub.extend_from_slice(subscription.as_bytes());
        self._inner
            .send(Message::Message(ZmqMessage { data: sub.freeze() }))
            .await?;
        Ok(())
    }
}
