use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::*;
use crate::transport::AcceptStopHandle;
use crate::util::{Peer, PeerIdentity};
use crate::*;
use crate::{SocketType, ZmqResult};

use async_trait::async_trait;
use crossbeam::queue::SegQueue;
use dashmap::DashMap;
use futures::lock::Mutex;
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;

struct ReqSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, Peer>,
    pub(crate) round_robin: SegQueue<PeerIdentity>,
    pub(crate) current_request_peer_id: Mutex<Option<PeerIdentity>>,
}

pub struct ReqSocket {
    backend: Arc<ReqSocketBackend>,
    current_request: Option<PeerIdentity>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for ReqSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl BlockingSend for ReqSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        if self.current_request.is_some() {
            return Err(ZmqError::ReturnToSender {
                reason: "Unable to send message. Request already in progress",
                message,
            });
        }
        // In normal scenario this will always be only 1 iteration
        // There can be special case when peer has disconnected and his id is still in
        // RR queue This happens because SegQueue don't have an api to delete
        // items from queue. So in such case we'll just pop item and skip it if
        // we don't have a matching peer in peers map
        loop {
            let next_peer_id = match self.backend.round_robin.pop() {
                Ok(peer) => peer,
                Err(_) => {
                    return Err(ZmqError::ReturnToSender {
                        reason: "Not connected to peers. Unable to send messages",
                        message,
                    })
                }
            };
            match self.backend.peers.get_mut(&next_peer_id) {
                Some(mut peer) => {
                    self.backend.round_robin.push(next_peer_id.clone());
                    let frames = vec![
                        "".into(), // delimiter frame
                        message,
                    ];
                    peer.send_queue.send(Message::Multipart(frames)).await?;
                    self.backend
                        .current_request_peer_id
                        .lock()
                        .await
                        .replace(next_peer_id.clone());
                    self.current_request = Some(next_peer_id);
                    return Ok(());
                }
                None => continue,
            }
        }
    }
}

#[async_trait]
impl BlockingRecv for ReqSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        match self.current_request.take() {
            Some(peer_id) => {
                if let Some(mut peer) = self.backend.peers.get_mut(&peer_id) {
                    let message = peer.recv_queue.next().await;
                    match message {
                        Some(Ok(Message::Multipart(mut message))) => {
                            assert!(message.len() == 2);
                            assert!(message[0].data.is_empty()); // Ensure that we have delimeter as first part
                            Ok(message.pop().unwrap())
                        }
                        Some(_) => Err(ZmqError::Other("Wrong message type received")),
                        None => Err(ZmqError::NoMessage),
                    }
                } else {
                    Err(ZmqError::Other("Server disconnected"))
                }
            }
            None => Err(ZmqError::Other("Unable to recv. No request in progress")),
        }
    }
}

#[async_trait]
impl Socket for ReqSocket {
    fn new() -> Self {
        Self {
            backend: Arc::new(ReqSocketBackend {
                peers: DashMap::new(),
                round_robin: SegQueue::new(),
                current_request_peer_id: Mutex::new(None),
            }),
            current_request: None,
            binds: HashMap::new(),
        }
    }

    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }

    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle> {
        &mut self.binds
    }
}

impl MultiPeerBackend for ReqSocketBackend {
    fn peer_connected(&self, peer_id: &PeerIdentity, io: FramedIo) {
        let (recv_queue, send_queue) = io.into_parts();
        self.peers.insert(
            peer_id.clone(),
            Peer {
                _identity: peer_id.clone(),
                send_queue,
                recv_queue,
            },
        );
        self.round_robin.push(peer_id.clone());
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }
}

#[async_trait]
impl SocketBackend for ReqSocketBackend {
    async fn message_received(&self, peer_id: &PeerIdentity, message: Message) {}

    fn socket_type(&self) -> SocketType {
        SocketType::REQ
    }

    fn shutdown(&self) {
        println!("Shutting down req backend");
        self.peers.clear();
    }
}
