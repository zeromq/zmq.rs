use crate::codec::*;
use crate::endpoint::{Endpoint, TryIntoEndpoint};
use crate::error::*;
use crate::fair_queue::FairQueue;
use crate::transport;
use crate::util::FairQueueProcessor;
use crate::*;
use crate::{util, SocketType, ZmqResult};

use async_trait::async_trait;
use dashmap::DashMap;
use futures::SinkExt;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;

struct RepPeer {
    pub(crate) _identity: PeerIdentity,
    pub(crate) send_queue: mpsc::Sender<Message>,
    pub(crate) recv_queue_in: mpsc::Sender<Message>,
    pub(crate) _io_close_handle: futures::channel::oneshot::Sender<bool>,
}

struct RepSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, RepPeer>,
    pub(crate) peer_queue_in: mpsc::Sender<(PeerIdentity, mpsc::Receiver<Message>)>,
}

pub struct RepSocket {
    backend: Arc<RepSocketBackend>,
    _fair_queue_close_handle: oneshot::Sender<bool>,
    current_request: Option<PeerIdentity>,
    fair_queue: mpsc::Receiver<(PeerIdentity, Message)>,
    binds: HashMap<Endpoint, oneshot::Sender<bool>>,
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

    async fn bind(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) -> ZmqResult<Endpoint> {
        let endpoint = endpoint.try_into()?;
        let Endpoint::Tcp(host, port) = endpoint;

        let cloned_backend = self.backend.clone();
        let cback = move |result| util::peer_connected(result, cloned_backend.clone());
        let (endpoint, stop_handle) = transport::tcp::begin_accept(host, port, cback).await?;

        self.binds.insert(endpoint.clone(), stop_handle);
        Ok(endpoint)
    }

    async fn connect(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) -> ZmqResult<()> {
        let endpoint = endpoint.try_into()?;
        let Endpoint::Tcp(host, port) = endpoint;

        let connect_result = transport::tcp::connect(host, port).await;
        util::peer_connected(connect_result, self.backend.clone()).await;
        Ok(())
    }

    fn binds(&self) -> &HashMap<Endpoint, oneshot::Sender<bool>> {
        &self.binds
    }
}

#[async_trait]
impl MultiPeer for RepSocketBackend {
    async fn peer_connected(
        &self,
        peer_id: &PeerIdentity,
    ) -> (mpsc::Receiver<Message>, oneshot::Receiver<bool>) {
        let default_queue_size = 100;
        let (out_queue, out_queue_receiver) = mpsc::channel(default_queue_size);
        let (in_queue, in_queue_receiver) = mpsc::channel::<Message>(default_queue_size);
        let (stop_handle, stop_callback) = oneshot::channel::<bool>();

        self.peers.insert(
            peer_id.clone(),
            RepPeer {
                _identity: peer_id.clone(),
                send_queue: out_queue,
                recv_queue_in: in_queue,
                _io_close_handle: stop_handle,
            },
        );
        self.peer_queue_in
            .clone()
            .try_send((peer_id.clone(), in_queue_receiver))
            .unwrap();

        (out_queue_receiver, stop_callback)
    }

    async fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }
}

#[async_trait]
impl SocketBackend for RepSocketBackend {
    async fn message_received(&self, peer_id: &PeerIdentity, message: Message) {
        self.peers
            .get_mut(peer_id)
            .expect("Not found peer by id")
            .recv_queue_in
            .send(message)
            .await
            .expect("Failed to send");
    }

    fn socket_type(&self) -> SocketType {
        SocketType::REP
    }

    fn shutdown(&self) {
        self.peers.clear();
    }
}

impl NonBlockingSend for RepSocket {
    fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        match self.current_request.take() {
            Some(peer_id) => {
                if let Some(mut peer) = self.backend.peers.get_mut(&peer_id) {
                    let frames = vec![
                        "".into(), // delimiter frame
                        message,
                    ];
                    peer.send_queue.try_send(Message::Multipart(frames))?;
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
