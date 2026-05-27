use crate::codec::{Message, ZmqFramedRead};
use crate::endpoint::Endpoint;
use crate::error::{ZmqError, ZmqResult};
use crate::fair_queue::FairQueue;
use crate::message::ZmqMessage;
use crate::reconnect::ReconnectHandle;
use crate::sub_backend::{
    connect_with_reconnect, SocketBinds, SubSocketBackend, SubscriptionMessageType,
};
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{
    MultiPeerBackend, Socket, SocketBackend, SocketEvent, SocketOptions, SocketRecv, SocketType,
};

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::StreamExt;

use std::collections::HashMap;
use std::sync::Arc;

pub struct SubSocket {
    backend: Arc<SubSocketBackend>,
    fair_queue: FairQueue<ZmqFramedRead, PeerIdentity>,
    binds: SocketBinds,
    /// Handles to background reconnection tasks
    reconnect_handles: Vec<ReconnectHandle>,
}

impl Drop for SubSocket {
    fn drop(&mut self) {
        // Shutdown all reconnection tasks
        for handle in self.reconnect_handles.drain(..) {
            handle.shutdown();
        }
        self.backend.shutdown();
    }
}

impl SubSocket {
    pub async fn subscribe(&mut self, subscription: &str) -> ZmqResult<()> {
        self.backend.remember_subscription(subscription.as_bytes());
        self.backend
            .broadcast_subscription(subscription.as_bytes(), SubscriptionMessageType::Subscribe)
            .await
    }

    pub async fn unsubscribe(&mut self, subscription: &str) -> ZmqResult<()> {
        self.backend.forget_subscription(subscription.as_bytes());
        self.backend
            .broadcast_subscription(
                subscription.as_bytes(),
                SubscriptionMessageType::Unsubscribe,
            )
            .await
    }
}

#[async_trait]
impl Socket for SubSocket {
    fn with_options(options: SocketOptions) -> Self {
        let mut fair_queue = FairQueue::new(true);
        let backend = Arc::new(SubSocketBackend::with_options(
            Some(fair_queue.inner()),
            SocketType::SUB,
            options,
        ));

        // Set callback to notify backend when a stream ends (peer disconnected)
        // Use Weak to avoid Arc cycle: backend -> fair_queue_inner -> callback -> backend
        let backend_weak = Arc::downgrade(&backend);
        fair_queue.set_on_disconnect(move |peer_id: PeerIdentity| {
            if let Some(backend) = backend_weak.upgrade() {
                backend.peer_disconnected(&peer_id);
            }
        });

        Self {
            backend,
            fair_queue,
            binds: HashMap::new(),
            reconnect_handles: Vec::new(),
        }
    }

    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }

    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle> {
        &mut self.binds
    }

    /// Connects to the given endpoint with automatic reconnection support.
    ///
    /// Unlike the default `Socket::connect`, this implementation spawns a
    /// background task that will automatically reconnect if the connection
    /// is lost. On reconnection, subscriptions are automatically re-sent
    /// to the peer.
    async fn connect(&mut self, endpoint: &str) -> ZmqResult<()> {
        connect_with_reconnect(self.backend.clone(), &mut self.reconnect_handles, endpoint).await
    }

    fn monitor(&mut self) -> mpsc::Receiver<SocketEvent> {
        let (sender, receiver) = mpsc::channel(1024);
        self.backend.socket_monitor.lock().replace(sender);
        receiver
    }
}

#[async_trait]
impl SocketRecv for SubSocket {
    #[inline]
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((_peer_id, Ok(Message::Message(message)))) => {
                    return Ok(message);
                }
                Some((_peer_id, Ok(_msg))) => {
                    // Ignore non-message frames. SUB sockets are designed to only receive actual messages,
                    // not internal protocol frames like commands or greetings.
                }
                Some((peer_id, Err(e))) => {
                    self.backend.peer_disconnected(&peer_id);
                    // Handle potential errors from the fair queue
                    return Err(e.into());
                }
                None => {
                    // The fair queue is empty, which shouldn't happen in normal operation
                    // this can happen if the peer disconnects while we are polling
                    return Err(ZmqError::NoMessage);
                }
            }
        }
    }
}
