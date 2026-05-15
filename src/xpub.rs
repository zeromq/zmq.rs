use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::ZmqResult;
use crate::fair_queue::{FairQueue, QueueInner};
use crate::message::*;
use crate::pub_fanout::{
    subscription_change, FanoutEvent, FanoutEventReceiver, FanoutEventSender, FanoutState,
};
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{CaptureSocket, SocketOptions};
use crate::{
    MultiPeerBackend, Socket, SocketBackend, SocketEvent, SocketRecv, SocketSend, SocketType,
    ZmqError,
};

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::StreamExt;
use parking_lot::Mutex;

use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct XPubSocketBackend {
    fanout_events: FanoutEventSender,
    fair_queue_inner: Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>,
    socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
    socket_options: SocketOptions,
}

impl SocketBackend for XPubSocketBackend {
    fn socket_type(&self) -> SocketType {
        SocketType::XPUB
    }

    fn socket_options(&self) -> &SocketOptions {
        &self.socket_options
    }

    fn shutdown(&self) {
        self.fair_queue_inner.lock().clear();
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

#[async_trait]
impl MultiPeerBackend for XPubSocketBackend {
    async fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        let (recv_queue, send_queue) = io.into_parts();

        if self
            .fanout_events
            .unbounded_send(FanoutEvent::PeerConnected {
                peer_id: peer_id.clone(),
                send_queue,
                fanout_events: self.fanout_events.clone(),
            })
            .is_err()
        {
            return;
        }

        self.fair_queue_inner
            .lock()
            .insert(peer_id.clone(), recv_queue);
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        log::info!("Client disconnected {:?}", peer_id);
        self.fair_queue_inner.lock().remove(peer_id);
        let _ = self
            .fanout_events
            .unbounded_send(FanoutEvent::PeerDisconnected(peer_id.clone()));
    }
}

pub struct XPubSocket {
    pub(crate) backend: Arc<XPubSocketBackend>,
    fair_queue: FairQueue<ZmqFramedRead, PeerIdentity>,
    fanout_state: FanoutState,
    fanout_events: FanoutEventReceiver,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for XPubSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl SocketSend for XPubSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        self.fanout_state.drain_events(&mut self.fanout_events);

        let first_frame = match message.get(0) {
            Some(frame) => frame,
            None => return Ok(()), // Empty message, nothing to publish
        };

        let dead_peers = self
            .fanout_state
            .send_matching(first_frame, &message)
            .await?;
        for peer in dead_peers {
            self.backend.peer_disconnected(&peer);
        }
        Ok(())
    }
}

#[async_trait]
impl SocketRecv for XPubSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        self.fanout_state.drain_events(&mut self.fanout_events);

        loop {
            match self.fair_queue.next().await {
                Some((peer_id, Ok(Message::Message(message)))) => {
                    self.fanout_state.drain_events(&mut self.fanout_events);
                    if let Some(change) = subscription_change(&message) {
                        self.fanout_state.apply_subscription(&peer_id, change);
                    }
                    return Ok(message);
                }
                Some((_peer_id, Ok(_msg))) => {
                    // Ignore non-message frames
                }
                Some((peer_id, Err(e))) => {
                    self.fanout_state.peer_disconnected(&peer_id);
                    self.backend.peer_disconnected(&peer_id);
                    return Err(e.into());
                }
                None => {
                    return Err(ZmqError::NoMessage);
                }
            }
        }
    }
}

impl CaptureSocket for XPubSocket {}

#[async_trait]
impl Socket for XPubSocket {
    fn with_options(options: SocketOptions) -> Self {
        let mut fair_queue = FairQueue::new(true);
        let (fanout_event_sender, fanout_events) = mpsc::unbounded();
        let backend = Arc::new(XPubSocketBackend {
            fanout_events: fanout_event_sender,
            fair_queue_inner: fair_queue.inner(),
            socket_monitor: Mutex::new(None),
            socket_options: options,
        });

        let backend_weak = Arc::downgrade(&backend);
        fair_queue.set_on_disconnect(move |peer_id: PeerIdentity| {
            if let Some(backend) = backend_weak.upgrade() {
                backend.peer_disconnected(&peer_id);
            }
        });

        Self {
            backend,
            fair_queue,
            fanout_state: FanoutState::default(),
            fanout_events,
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
        let s = XPubSocket::new();
        test_bind_to_any_port_helper(s).await
    }

    #[async_rt::test]
    async fn test_bind_to_any_ipv4_interface() -> ZmqResult<()> {
        let any_ipv4: IpAddr = "0.0.0.0".parse().unwrap();
        let s = XPubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv4, s, 4020).await
    }

    #[async_rt::test]
    async fn test_bind_to_any_ipv6_interface() -> ZmqResult<()> {
        let any_ipv6: IpAddr = "::".parse().unwrap();
        let s = XPubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv6, s, 4030).await
    }
}
