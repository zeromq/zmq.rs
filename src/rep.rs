use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::*;
use crate::fair_queue::{FairQueue, QueueInner};
use crate::transport::AcceptStopHandle;
use crate::*;
use crate::{SocketType, ZmqResult};
use async_trait::async_trait;
use dashmap::DashMap;
use futures::SinkExt;
use futures::StreamExt;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

struct RepPeer {
    pub(crate) _identity: PeerIdentity,
    pub(crate) send_queue: ZmqFramedWrite,
}

struct RepSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, RepPeer>,
    fair_queue_inner: Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>,
    socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
}

pub struct RepSocket {
    backend: Arc<RepSocketBackend>,
    current_request: Option<PeerIdentity>,
    fair_queue: FairQueue<ZmqFramedRead, PeerIdentity>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for RepSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl Socket for RepSocket {
    fn new() -> Self {
        let fair_queue = FairQueue::new(true);
        Self {
            backend: Arc::new(RepSocketBackend {
                peers: DashMap::new(),
                fair_queue_inner: fair_queue.inner(),
                socket_monitor: Mutex::new(None),
            }),
            current_request: None,
            fair_queue,
            binds: HashMap::new(),
        }
    }

    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }

    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle> {
        &mut self.binds
    }

    fn monitor(&mut self) -> mpsc::Receiver<SocketEvent> {
        let (sender, receiver) = mpsc::channel(1024);
        self.backend.socket_monitor.lock().replace(sender);
        receiver
    }
}

impl MultiPeerBackend for RepSocketBackend {
    fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        let (recv_queue, send_queue) = io.into_parts();

        self.peers.insert(
            peer_id.clone(),
            RepPeer {
                _identity: peer_id.clone(),
                send_queue,
            },
        );
        self.fair_queue_inner
            .lock()
            .insert(peer_id.clone(), recv_queue);
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        if let Some(monitor) = self.monitor().lock().as_mut() {
            let _ = monitor.try_send(SocketEvent::Disconnected(peer_id.clone()));
        }
        self.peers.remove(peer_id);
    }
}

impl SocketBackend for RepSocketBackend {
    fn socket_type(&self) -> SocketType {
        SocketType::REP
    }

    fn shutdown(&self) {
        self.peers.clear();
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

#[async_trait]
impl BlockingSend for RepSocket {
    async fn send(&mut self, message: Message) -> ZmqResult<()> {
        match self.current_request.take() {
            Some(peer_id) => {
                if let Some(mut peer) = self.backend.peers.get_mut(&peer_id) {
                    let message = match message {
                        Message::Message(m) => {
                            let frames = vec![
                                "".into(), // delimiter frame
                                m,
                            ];
                            Message::Multipart(frames)
                        }
                        Message::Multipart(mut messages) => {
                            messages.insert(0, "".into());
                            Message::Multipart(messages)
                        }
                        _ => todo!(),
                    };
                    peer.send_queue.send(message).await?;
                    Ok(())
                } else {
                    Err(ZmqError::ReturnToSender {
                        reason: "Client disconnected",
                        message,
                    })
                }
            }
            None => Err(ZmqError::ReturnToSender {
                reason: "Unable to send reply. No request in progress",
                message,
            }),
        }
    }
}

#[async_trait]
impl BlockingRecv for RepSocket {
    async fn recv(&mut self) -> ZmqResult<Message> {
        loop {
            match self.fair_queue.next().await {
                Some((peer_id, Ok(Message::Multipart(mut messages)))) => {
                    self.current_request = Some(peer_id);
                    return match messages.len() {
                        0 | 1 => Err(ZmqError::Other("Malformed reply message")),
                        2 => {
                            // Ensure that we have delimeter as first part
                            assert!(messages[0].data.is_empty());
                            Ok(Message::Message(messages.pop().unwrap()))
                        }
                        _ => {
                            // Ensure that we have delimeter as first part
                            assert!(messages[0].data.is_empty());
                            Ok(Message::Multipart(messages[1..].to_vec()))
                        }
                    };
                }
                Some((_peer_id, _)) => todo!(),
                None => return Err(ZmqError::NoMessage),
            };
        }
    }
}
