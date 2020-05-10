use async_trait::async_trait;
use futures::{select, Future, SinkExt};
use futures::channel::mpsc::*;
use futures::stream::StreamExt;
use futures_util::future::FutureExt;
use tokio::net::TcpStream;
use tokio::stream::{Stream};
use tokio_util::codec::Framed;

use crate::codec::*;
use crate::error::*;
use crate::util::*;
use crate::{Socket, SocketType, ZmqResult};
use dashmap::DashMap;
use std::sync::Arc;

pub(crate) struct Peer {
    pub(crate) identity: PeerIdentity,
    _send_queue: futures::channel::mpsc::Sender<Message>,
    _recv_queue: futures::channel::mpsc::Receiver<Message>,
    _io_close_handle: futures::channel::oneshot::Sender<bool>,
}

pub struct RouterSocket {
    pub(crate) peers: Arc<DashMap<PeerIdentity, Peer>>,
    _accept_close_handle: futures::channel::oneshot::Sender<bool>,
}

impl Drop for RouterSocket {
    fn drop(&mut self) {
        self.peers.clear()
    }
}

impl RouterSocket {
    async fn peer_connected(
        socket: tokio::net::TcpStream,
        peers: Arc<DashMap<PeerIdentity, Peer>>,
    ) {
        let (read, write) = tokio::io::split(socket);
        let mut read_part = tokio_util::codec::FramedRead::new(read, ZmqCodec::new());
        let mut write_part = tokio_util::codec::FramedWrite::new(write, ZmqCodec::new());

        greet_exchange_w_parts(&mut write_part, &mut read_part)
            .await
            .expect("Failed to exchange greetings");

        let peer_id = ready_exchange_w_parts(&mut write_part, &mut read_part, SocketType::ROUTER)
            .await
            .expect("Failed to exchange ready messages");
        println!("Peer connected {:?}", peer_id);

        let default_queue_size = 100;
        let (_send_queue, _send_queue_receiver) = futures::channel::mpsc::channel(default_queue_size);
        let (mut _recv_queue, _recv_queue_receiver) = futures::channel::mpsc::channel(default_queue_size);
        let (sender, receiver) = futures::channel::oneshot::channel::<bool>();

        let peer = Peer {
            identity: peer_id.clone(),
            _send_queue: _send_queue,
            _recv_queue: _recv_queue_receiver,
            _io_close_handle: sender,
        };

        peers.insert(peer_id.clone(), peer);

        let mut stop_handle = receiver.fuse();
        //let mut write_part = write_part.fuse();
        let mut messages = vec![ZmqMessage { data: peer_id.clone().into(), more: true}];
        let mut incoming_queue = read_part.fuse();
        let mut outgoing_queue = _send_queue_receiver.fuse();
        loop {
            select!{
                incoming = incoming_queue.next() => {
                    match incoming {
                        Some(Ok(Message::Message(message))) => {
                            dbg!(&message);
                            let more = message.more;
                            messages.push(message);
                            if !more {
                                _recv_queue.send(Message::MultipartMessage(messages)).await;
                                messages = vec![ZmqMessage { data: peer_id.clone().into(), more: true}];
                            }
                        }
                        None => {
                            println!("Client disconnected {:?}", &peer_id);
                            peers.remove(&peer_id);
                            break;
                        }
                        _ => todo!(),
                    }
                },
                outgoing = outgoing_queue.next() => {
                    match outgoing {
                        Some(message) => {
                            let result = write_part.send(message).await;
                            dbg!(result);
                        },
                        None => {
                            println!("Outgoing queue closed. Stopping send coro");
                            break;
                        }
                    }
                },
                _ = stop_handle => {
                    println!("Stop callback received");
                    break;
                }
            }
        }
    }

    pub async fn bind(endpoint: &str) -> ZmqResult<Self> {
        let mut listener = tokio::net::TcpListener::bind(endpoint).await?;
        let (sender, receiver) = futures::channel::oneshot::channel::<bool>();
        let router_socket = Self {
            peers: Arc::new(DashMap::new()),
            _accept_close_handle: sender,
        };
        let peers = router_socket.peers.clone();
        let f = RouterSocket::peer_connected;
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

    pub async fn recv_multipart(&mut self) -> ZmqResult<Vec<ZmqMessage>> {
        for mut peer in self.peers.iter_mut() {
            match peer.value_mut()._recv_queue.try_next() {
                Ok(Some(Message::MultipartMessage(messages))) => {
                    return Ok(messages)
                },
                Err(TryRecvError) => {
                    continue
                }
                _ => todo!(),
            }
        }
        Err(ZmqError::NoMessage)
    }

    pub async fn send_multipart(&mut self, messages: Vec<ZmqMessage>) -> ZmqResult<()> {
        assert!(messages.len() > 2);
        let peer_id: PeerIdentity = messages[0].data.to_vec().into();
        match self.peers.get_mut(&peer_id) {
            Some(mut peer) => {
                peer._send_queue.try_send(Message::MultipartMessage(messages[1..].to_vec()))?;
                Ok(())
            },
            None => {
                return Err(ZmqError::Other("Destination client not found by identity"))
            }
        }
    }
}

#[async_trait]
impl Socket for RouterSocket {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()> {
        unimplemented!()
    }

    async fn recv(&mut self) -> ZmqResult<Vec<u8>> {
        for mut peer in self.peers.iter_mut() {
            match peer.value_mut()._recv_queue.try_next() {
                Ok(Some(Message::MultipartMessage(messages))) => {
                    let mut data = Vec::new();
                    for m in messages {
                        data.extend(m.data.to_vec());
                    }
                    return Ok(data)
                },
                Err(TryRecvError) => {
                    continue
                }
                _ => todo!(),
            }
        }
        Err(ZmqError::NoMessage)
    }
}

pub struct DealerSocket {
    pub(crate) _inner: Framed<TcpStream, ZmqCodec>,
}

impl DealerSocket {
    pub async fn bind(_endpoint: &str) -> ZmqResult<Self> {
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
