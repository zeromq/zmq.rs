use crate::codec::*;
use crate::error::*;
use crate::fair_queue::FairQueue;
use crate::*;
use crate::{SocketType, ZmqResult};
use async_trait::async_trait;
use dashmap::DashMap;
use futures_util::sink::SinkExt;
use std::sync::Arc;
use tokio::stream::StreamExt;


struct RepPeer {
    pub(crate) identity: PeerIdentity,
    pub(crate) send_queue: mpsc::Sender<Message>,
    pub(crate) recv_queue_in: mpsc::Sender<Message>,
    pub(crate) _io_close_handle: futures::channel::oneshot::Sender<bool>,
}

struct FairQueueProcessor {
    pub(crate) fair_queue_stream: FairQueue<mpsc::Receiver<Message>, PeerIdentity>,
    pub(crate) socket_incoming_queue: mpsc::Sender<(PeerIdentity, Message)>,
    pub(crate) peer_queue_in: mpsc::Receiver<(PeerIdentity, mpsc::Receiver<Message>)>,
    pub(crate) _io_close_handle: oneshot::Receiver<bool>,
}

struct RepSocketBackend {
    pub(crate) peers: DashMap<PeerIdentity, RepPeer>,
    pub(crate) peer_queue_in: mpsc::Sender<(PeerIdentity, mpsc::Receiver<Message>)>,
}

pub struct RepSocket {
    backend: Arc<RepSocketBackend>,
    _accept_close_handle: Option<oneshot::Sender<bool>>,
    fair_queue_close_handle: oneshot::Sender<bool>,
    current_request: Option<PeerIdentity>,
    fair_queue: mpsc::Receiver<(PeerIdentity, Message)>,
}

#[async_trait]
impl SocketFrontend for RepSocket {
    fn new() -> Self {
        // TODO define buffer size
        let default_queue_size = 100;
        let (queue_sender, fair_queue) = mpsc::channel(default_queue_size);
        let (peer_in, peer_out) = mpsc::channel(default_queue_size);
        let (fair_queue_close_handle, fqueue_close_recevier) = oneshot::channel();
        tokio::spawn(process_fair_queue_messages(FairQueueProcessor {
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
            _accept_close_handle: None,
            fair_queue_close_handle,
            current_request: None,
            fair_queue,
        }
    }

    async fn bind(&mut self, endpoint: &str) -> ZmqResult<()> {
        let stop_handle = util::start_accepting_connections(endpoint, self.backend.clone()).await?;
        self._accept_close_handle = Some(stop_handle);
        Ok(())
    }

    async fn connect(&mut self, endpoint: &str) -> ZmqResult<()> {
        let addr = endpoint.parse::<SocketAddr>()?;
        let raw_socket = tokio::net::TcpStream::connect(addr).await?;
        util::peer_connected(raw_socket, self.backend.clone()).await;
        Ok(())
    }
}

async fn process_fair_queue_messages(mut processor: FairQueueProcessor) {
    use futures::future::FutureExt;
    let mut stop_callback = processor._io_close_handle.fuse();
    let mut waiting_for_clients = true;
    let mut waiting_for_data = true;
    loop {
        tokio::select! {
            _ = &mut stop_callback => {
                println!("Socket dropped. stop fair_queue");
                break;
            },
            peer_in = processor.peer_queue_in.next().fuse(), if waiting_for_clients => {
                println!("Insert new stream");
                match peer_in {
                    Some((peer_id, receiver)) => {
                        processor.fair_queue_stream.insert(peer_id, receiver);
                        waiting_for_data = true;
                    },
                    None => {
                        // Channel for newly connected clients was closed
                        // so we no longer wait for the to arrive
                        waiting_for_clients = false;
                    },
                };
            },
            message = processor.fair_queue_stream.next().fuse(), if waiting_for_data => {
                match message {
                    Some(m) => {
                        processor.socket_incoming_queue.send(m).await;
                    },
                    None => {
                        // This is the case when there are no connected clients
                        // We should sleep and wait for new clients to connect
                        // This is handled by 2nd branch of select
                        if waiting_for_clients {
                            waiting_for_data = false;
                        } else {
                            // We're not waiting for client and have no data...
                            // stop the loop and cleanup
                            break;
                        };
                    }
                };
            }
        }
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
                identity: peer_id.clone(),
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
                    peer.send_queue
                        .send(Message::MultipartMessage(frames))
                        .await?;
                    Ok(())
                } else {
                    Err(ZmqError::Other("Client disconnected"))
                }
            }
            None => Err(ZmqError::Other(
                "Unable to send reply. No request in progress",
            )),
        }
    }
}

#[async_trait]
impl BlockingRecv for RepSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((peer_id, Message::MultipartMessage(mut messages))) => {
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
