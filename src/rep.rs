use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::*;
use crate::fair_queue::FairQueue;
use crate::transport::AcceptStopHandle;
use crate::util::FairQueueProcessor;
use crate::*;
use crate::{util, SocketType, ZmqResult};

use async_trait::async_trait;
use dashmap::DashMap;
use futures::SinkExt;
use futures::StreamExt;
use futures_codec::FramedRead;
use std::collections::HashMap;
use std::sync::Arc;

struct RepPeer {
    pub(crate) _identity: PeerIdentity,
    pub(crate) send_queue: FramedWrite<Box<dyn FrameableWrite>, ZmqCodec>,
}

struct RepSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, RepPeer>,
    pub(crate) peer_queue_in:
        mpsc::Sender<(PeerIdentity, FramedRead<Box<dyn FrameableRead>, ZmqCodec>)>,
}

pub struct RepSocket {
    backend: Arc<RepSocketBackend>,
    _fair_queue_close_handle: oneshot::Sender<bool>,
    current_request: Option<PeerIdentity>,
    fair_queue: mpsc::Receiver<(PeerIdentity, Message)>,
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
        // TODO define buffer size
        let default_queue_size = 100;
        let (queue_sender, fair_queue) = mpsc::channel(default_queue_size);
        let (peer_in, peer_out) = mpsc::channel(default_queue_size);
        let (fair_queue_close_handle, fqueue_close_recevier) = oneshot::channel();
        tokio::spawn(util::process_fair_queue_messages(FairQueueProcessor {
            fair_queue_stream: FairQueue::new(),
            socket_incoming_queue: queue_sender,
            peer_queue_in: peer_out,
            _io_close_handle: fqueue_close_recevier,
        }));
        Self {
            backend: Arc::new(RepSocketBackend {
                peers: DashMap::new(),
                peer_queue_in: peer_in,
            }),
            _fair_queue_close_handle: fair_queue_close_handle,
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
}

impl MultiPeerBackend for RepSocketBackend {
    fn peer_connected(&self, peer_id: &PeerIdentity, io: FramedIo) {
        let (recv_queue, send_queue) = io.into_parts();

        self.peers.insert(
            peer_id.clone(),
            RepPeer {
                _identity: peer_id.clone(),
                send_queue,
            },
        );
        self.peer_queue_in
            .clone()
            .try_send((peer_id.clone(), recv_queue))
            .unwrap();
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }
}

#[async_trait]
impl SocketBackend for RepSocketBackend {
    async fn message_received(&self, peer_id: &PeerIdentity, message: Message) {}

    fn socket_type(&self) -> SocketType {
        SocketType::REP
    }

    fn shutdown(&self) {
        self.peers.clear();
    }
}

#[async_trait]
impl BlockingSend for RepSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        match self.current_request.take() {
            Some(peer_id) => {
                if let Some(mut peer) = self.backend.peers.get_mut(&peer_id) {
                    let frames = vec![
                        "".into(), // delimiter frame
                        message,
                    ];
                    peer.send_queue.send(Message::Multipart(frames)).await?;
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
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((peer_id, Message::Multipart(mut messages))) => {
                    assert!(messages.len() == 2);
                    assert!(messages[0].data.is_empty()); // Ensure that we have delimeter as first part
                    self.current_request = Some(peer_id);
                    return Ok(messages.pop().unwrap());
                }
                Some((_peer_id, _)) => todo!(),
                None => todo!(),
            };
        }
    }
}
