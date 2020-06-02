use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};
use futures::select;
use futures::FutureExt;
use futures::SinkExt;
use futures::StreamExt;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::message::*;
use crate::util::*;
use crate::{MultiPeer, Socket, SocketBackend, SocketType, ZmqResult};
use bytes::{BufMut, BytesMut};
use dashmap::DashMap;
use futures::lock::Mutex;
use std::sync::Arc;

pub(crate) struct Subscriber {
    pub subscriptions: Vec<Vec<u8>>,
    pub peer: Peer,
}

pub(crate) struct PubSocketBackend {
    subscribers: Arc<DashMap<PeerIdentity, Subscriber>>,
}

#[async_trait]
impl SocketBackend for PubSocketBackend {
    async fn message_received(&self, peer_id: &PeerIdentity, message: ZmqMessage) {
        let data: Vec<u8> = message.into();
        if data.len() < 1 {
            panic!("Unable to handle message")
        }
        match data[0] {
            1 => {
                // Subscribe
                self.subscribers
                    .get_mut(&peer_id)
                    .unwrap()
                    .subscriptions
                    .push(Vec::from(&data[1..]));
            }
            0 => {
                // Unsubscribe
                let mut del_index = None;
                let sub = Vec::from(&data[1..]);
                for (idx, subscription) in self
                    .subscribers
                    .get(&peer_id)
                    .unwrap()
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
                    self.subscribers
                        .get_mut(&peer_id)
                        .unwrap()
                        .subscriptions
                        .remove(index);
                }
            }
            _ => panic!("Malformed message"),
        }
    }
}

impl MultiPeer for PubSocketBackend {
    fn peer_connected(
        &self,
        peer_id: &PeerIdentity,
    ) -> (mpsc::Receiver<Message>, oneshot::Receiver<bool>) {
        let default_queue_size = 100;
        let (out_queue, out_queue_receiver) = mpsc::channel(default_queue_size);
        let (mut in_queue, in_queue_receiver) = mpsc::channel(default_queue_size);
        let (stop_handle, stop_callback) = oneshot::channel::<bool>();

        let peer = Peer {
            identity: peer_id.clone(),
            send_queue: out_queue,
            recv_queue: Arc::new(Mutex::new(in_queue_receiver)), // TODO this socket doesn't have recv queue
            _io_close_handle: stop_handle,
        };

        self.subscribers.insert(
            peer_id.clone(),
            Subscriber {
                subscriptions: vec![],
                peer: peer,
            },
        );
        (out_queue_receiver, stop_callback)
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        println!("Client disconnected {:?}", peer_id);
        self.subscribers.remove(peer_id);
    }
}

pub struct PubSocket {
    _accept_close_handle: futures::channel::oneshot::Sender<bool>,
    pub(crate) backend: Arc<PubSocketBackend>,
}

#[async_trait]
impl Socket for PubSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        for mut subscriber in self.backend.subscribers.iter_mut() {
            for sub_filter in &subscriber.subscriptions {
                if sub_filter.as_slice() == &message.data[0..sub_filter.len()] {
                    subscriber
                        .peer
                        .send_queue
                        .try_send(Message::Message(message.clone()));
                    break;
                }
            }
        }
        Ok(())
    }

    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        Err(ZmqError::Socket(
            "This socket doesn't support receiving messages",
        ))
    }
}

impl PubSocket {
    async fn handle_subscriber(
        socket: tokio::net::TcpStream,
        subscribers: Arc<DashMap<PeerIdentity, Subscriber>>,
        backend: Arc<PubSocketBackend>,
    ) {
        let mut raw_socket = Framed::new(socket, ZmqCodec::new());

        greet_exchange(&mut raw_socket)
            .await
            .expect("Failed to exchange greetings");
        let peer_id = ready_exchange(&mut raw_socket, SocketType::PUB)
            .await
            .expect("Failed to exchange ready messages");
        println!("Peer connected {:?}", peer_id);

        let parts = raw_socket.into_parts();
        let (read, write) = tokio::io::split(parts.io);
        let mut read_part = tokio_util::codec::FramedRead::new(read, parts.codec);
        let mut write_part = tokio_util::codec::FramedWrite::new(write, ZmqCodec::new());

        let (outgoing_queue, stop_callback) = backend.peer_connected(&peer_id);

        let mut stop_callback = stop_callback.fuse();
        let mut incoming_queue = read_part.fuse();
        let mut outgoing_queue = outgoing_queue.fuse();
        loop {
            futures::select! {
                _ = stop_callback => {
                    println!("Stop callback received");
                    break;
                },
                outgoing = outgoing_queue.next() => {
                    match outgoing {
                        Some(message) => {
                            let result = write_part.send(message).await;
                            dbg!(result); // TODO add errors processing
                        },
                        None => {
                            println!("Outgoing queue closed. Stopping send coro");
                            break;
                        }
                    }
                },
                incoming = incoming_queue.next() => {
                    match incoming {
                        Some(Ok(Message::Message(message))) => {
                            backend.message_received(&peer_id, message).await;
                        }
                        None => {
                            backend.peer_disconnected(&peer_id);
                            break;
                        }
                        _ => todo!(),
                    }
                },
            }
        }
    }

    pub async fn bind(endpoint: &str) -> ZmqResult<Self> {
        let mut listener = tokio::net::TcpListener::bind(endpoint).await?;
        let (sender, receiver) = futures::channel::oneshot::channel::<bool>();
        let socket_backend = Arc::new(PubSocketBackend {
            subscribers: Arc::new(DashMap::new()),
        });
        let pub_socket = Self {
            _accept_close_handle: sender,
            backend: socket_backend.clone(),
        };
        let subscribers = pub_socket.backend.subscribers.clone();
        tokio::spawn(async move {
            let mut stop_callback = receiver.fuse();
            loop {
                select! {
                    incoming = listener.accept().fuse() => {
                        let (socket, _) = incoming.expect("Failed to accept connection");
                        tokio::spawn(PubSocket::handle_subscriber(socket, subscribers.clone(), socket_backend.clone()));
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
    async fn send(&mut self, _m: ZmqMessage) -> ZmqResult<()> {
        Err(ZmqError::Socket(
            "This socket doesn't support sending messages",
        ))
    }

    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        let message: Option<ZmqResult<Message>> = self._inner.next().await;
        match message {
            Some(Ok(Message::Message(m))) => Ok(m),
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
