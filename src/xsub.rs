use crate::codec::{Message, ZmqFramedRead};
use crate::error::{ZmqError, ZmqResult};
use crate::fair_queue::FairQueue;
use crate::sub_backend::{connect_with_reconnect, SubSocketBackend, SubscriptionMessageType};
use crate::util::PeerIdentity;
use crate::{
    MultiPeerBackend, Socket, SocketBackend, SocketBinds, SocketConnects, SocketEvent,
    SocketOptions, SocketRecv, SocketSend, SocketType,
};

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::StreamExt;

use std::collections::HashMap;
use std::sync::Arc;

pub struct XSubSocket {
    backend: Arc<SubSocketBackend>,
    fair_queue: FairQueue<ZmqFramedRead, PeerIdentity>,
    binds: SocketBinds,
    connects: SocketConnects,
}

impl Drop for XSubSocket {
    fn drop(&mut self) {
        // Shutdown all reconnection tasks
        for (_, stop_handle) in self.connects.drain() {
            if let Some(reconnect) = stop_handle.0 {
                reconnect.shutdown();
            }
        }
        self.backend.shutdown();
    }
}

impl XSubSocket {
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
impl Socket for XSubSocket {
    fn with_options(options: SocketOptions) -> Self {
        let mut fair_queue = FairQueue::new(true);
        let backend = Arc::new(SubSocketBackend::with_options(
            Some(fair_queue.inner()),
            SocketType::XSUB,
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
            connects: HashMap::new(),
        }
    }

    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }

    fn binds(&mut self) -> &mut SocketBinds {
        &mut self.binds
    }

    fn connects(&mut self) -> &mut SocketConnects {
        &mut self.connects
    }

    /// Connects to the given endpoint with automatic reconnection support.
    ///
    /// Unlike the default `Socket::connect`, this implementation spawns a
    /// background task that will automatically reconnect if the connection
    /// is lost. On reconnection, subscriptions are automatically re-sent
    /// to the peer.
    async fn connect(&mut self, endpoint: &str) -> ZmqResult<()> {
        connect_with_reconnect(self.backend.clone(), &mut self.connects, endpoint).await
    }

    fn monitor(&mut self) -> mpsc::Receiver<SocketEvent> {
        let (sender, receiver) = mpsc::channel(1024);
        self.backend.socket_monitor.lock().replace(sender);
        receiver
    }
}

#[async_trait]
impl SocketRecv for XSubSocket {
    async fn recv(&mut self) -> ZmqResult<crate::ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((_peer_id, Ok(Message::Message(message)))) => return Ok(message),
                Some((_peer_id, Ok(_msg))) => {
                    // Ignore non-message frames (commands, greetings, etc.)
                }
                Some((peer_id, Err(e))) => {
                    self.backend.peer_disconnected(&peer_id);
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

#[async_trait]
impl SocketSend for XSubSocket {
    async fn send(&mut self, message: crate::ZmqMessage) -> ZmqResult<()> {
        // If the message is a subscription frame (0x01/0x00 prefix), update
        // local state and send it reliably to all upstream peers.
        // Otherwise, treat it as a normal application message and fanout.
        if self.backend.apply_subscription_message(&message) {
            self.backend.broadcast_message_reliably(message).await
        } else {
            self.backend.fanout_message(message).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_rt;
    use crate::util::tests::{
        test_bind_to_any_port_helper, test_bind_to_unspecified_interface_helper,
    };
    use crate::ZmqResult;
    use std::net::IpAddr;

    #[async_rt::test]
    async fn test_bind_to_any_port() -> ZmqResult<()> {
        let s = XSubSocket::new();
        test_bind_to_any_port_helper(s).await
    }

    #[async_rt::test]
    async fn test_bind_to_any_ipv4_interface() -> ZmqResult<()> {
        let any_ipv4: IpAddr = "0.0.0.0".parse().unwrap();
        let s = XSubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv4, s, 4040).await
    }

    #[async_rt::test]
    async fn test_bind_to_any_ipv6_interface() -> ZmqResult<()> {
        let any_ipv6: IpAddr = "::".parse().unwrap();
        let s = XSubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv6, s, 4050).await
    }
}
