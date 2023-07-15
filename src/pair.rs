/// Implements the ZMQ EXPAIR communication strategy.
///
/// See RFC 31 https://rfc.zeromq.org/spec/31/
///
/// The PAIR Socket Type
///
/// General behavior:
///
/// - MAY be connected to at most one PAIR peers, and MAY both send and receive messages.
/// - SHALL not filter or modify outgoing or incoming messages in any way.
/// - SHALL maintain a double queue for its peer, allowing outgoing and incoming messages to be queued independently.
/// - SHALL create a double queue when initiating an outgoing connection to a peer, and SHALL maintain the double queue whether or not the connection is established.
/// - SHALL create a double queue when a peer connects to it. If this peer disconnects, the PAIR socket SHALL destroy its double queue and SHALL discard any messages it contains.
/// - SHOULD constrain incoming and outgoing queue sizes to a runtime-configurable limit.
///
/// For processing outgoing messages:
///
/// - SHALL consider its peer as available only when it has a outgoing queue that is not full.
/// - SHALL block on sending, or return a suitable error, when it has no available peer.
/// - SHALL not accept further messages when it has no available peer.
/// - SHALL NOT discard messages that it cannot queue.
///
/// For processing incoming messages:
///
/// - SHALL receive incoming messages from its single peer if it has one.
/// - SHALL deliver these to its calling application.
///
/// Example usage can be found in ../tests/pair.rs
///
use crate::async_rt::block_on_read_till_set::BlockOnReadTillSet;
use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::*;
use crate::transport::AcceptStopHandle;
use crate::util::{Peer, PeerIdentity};
use crate::*;
use crate::{SocketType, ZmqResult};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

struct PairSocketBackend {
    pub(crate) peer: BlockOnReadTillSet<Peer>,
    socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
    socket_options: SocketOptions,
    drop_tx: broadcast::Sender<()>,
}

pub struct PairSocket {
    backend: Arc<PairSocketBackend>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}


impl SocketBackend for PairSocketBackend {
    fn socket_type(&self) -> SocketType {
        SocketType::PAIR
    }

    fn socket_options(&self) -> &SocketOptions {
        &self.socket_options
    }

    fn shutdown(&self) {
        self.drop_tx.send(()).unwrap();
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

#[async_trait]
impl SocketSend for PairSocket {
    async fn send(&mut self, msg: ZmqMessage) -> ZmqResult<()> {
        return match *self.backend.peer.get().await {
            Some(ref mut peer) => peer
                .send_queue
                .send(Message::Message(msg))
                .await
                .map_err(|codec_error| ZmqError::Codec(codec_error)),
            None => panic!("Unreachable"),
        };
    }
}

#[async_trait]
impl SocketRecv for PairSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        let message = match *self.backend.peer.get().await {
            Some(ref mut peer) => peer.recv_queue.next().await,
            None => panic!("Unreachable"),
        };
        match message {
            Some(Ok(Message::Greeting(_))) => todo!(),
            Some(Ok(Message::Command(_))) => todo!(),
            Some(Ok(Message::Message(m))) => Ok(m),
            Some(Err(e)) => Err(ZmqError::Codec(e)),
            None => Err(ZmqError::NoMessage),
        }
    }
}

#[async_trait]
impl Socket for PairSocket {
    fn with_options(options: SocketOptions) -> Self {
        let (drop_tx, _) = broadcast::channel(100);
        Self {
            backend: Arc::new(PairSocketBackend {
                peer: BlockOnReadTillSet::new(),
                socket_monitor: Mutex::new(None),
                socket_options: options,
                drop_tx,
            }),
            binds: HashMap::new(),
        }
    }

    /// Bind to an endpoint & launch dropper task
    /// Calls the default bind internally
    async fn bind(&mut self, endpoint: &str) -> ZmqResult<Endpoint> {
        // Spawn dropper task
        let mut drop_rx = self.backend.drop_tx.subscribe();
        let peer = self.backend.peer.clone();
        tokio::spawn(async move {
            let _ = drop_rx.recv().await;
            peer.unset().await;
            panic!("Peer disconnected");
        });
        let ret = self.bind_default(endpoint).await;
        ret
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

#[async_trait]
impl MultiPeerBackend for PairSocketBackend {
    async fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        if self.peer.is_set().await {
            todo!("Refuse connection if already connected.");
        }
        let (recv_queue, send_queue) = io.into_parts();
        let peer = Peer {
            _identity: peer_id.clone(),
            send_queue: send_queue,
            recv_queue: recv_queue,
        };
        self.peer.set(peer).await;
    }

    fn peer_disconnected(&self, _peer_id: &PeerIdentity) {
        self.shutdown();
    }
}
