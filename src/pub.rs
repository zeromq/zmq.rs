use crate::codec::*;
use crate::endpoint::Endpoint;
use crate::error::ZmqResult;
use crate::message::*;
use crate::pub_fanout::{
    subscription_change, FanoutEvent, FanoutEventReceiver, FanoutEventSender, FanoutState,
};
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{async_rt, CaptureSocket, SocketOptions};
use crate::{MultiPeerBackend, Socket, SocketBackend, SocketEvent, SocketSend, SocketType};

use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};
use futures::{select, FutureExt, StreamExt};
use parking_lot::Mutex;

use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct PubConnection {
    _subscription_coro_stop: oneshot::Sender<()>,
}

pub(crate) struct PubSocketBackend {
    connections: scc::HashMap<PeerIdentity, PubConnection>,
    fanout_events: FanoutEventSender,
    socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
    socket_options: SocketOptions,
}

impl PubSocketBackend {
    fn message_received(&self, peer_id: &PeerIdentity, message: Message) {
        let message = match message {
            Message::Message(m) => m,
            _ => return,
        };

        if let Some(change) = subscription_change(&message) {
            let _ = self
                .fanout_events
                .unbounded_send(FanoutEvent::Subscription {
                    peer_id: peer_id.clone(),
                    change,
                });
            return;
        }

        if message.len() != 1 {
            log::warn!("Received message with unexpected length: {}", message.len());
            return;
        }

        let Some(frame) = message.get(0) else {
            return;
        };
        if !frame.is_empty() {
            log::warn!(
                "Received message with unexpected first byte: {:?}",
                frame.first()
            );
        }
    }
}

impl SocketBackend for PubSocketBackend {
    fn socket_type(&self) -> SocketType {
        SocketType::PUB
    }

    fn socket_options(&self) -> &SocketOptions {
        &self.socket_options
    }

    fn shutdown(&self) {
        self.connections.clear_sync();
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

#[async_trait]
impl MultiPeerBackend for PubSocketBackend {
    async fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        let (mut recv_queue, send_queue) = io.into_parts();
        let (sender, stop_receiver) = oneshot::channel();
        self.connections
            .upsert_async(
                peer_id.clone(),
                PubConnection {
                    _subscription_coro_stop: sender,
                },
            )
            .await;

        if self
            .fanout_events
            .unbounded_send(FanoutEvent::PeerConnected {
                peer_id: peer_id.clone(),
                send_queue,
                fanout_events: self.fanout_events.clone(),
            })
            .is_err()
        {
            self.connections.remove_sync(peer_id);
            return;
        }

        let backend = self;
        let peer_id = peer_id.clone();
        async_rt::task::spawn(async move {
            let mut stop_receiver = stop_receiver.fuse();
            loop {
                select! {
                     _ = stop_receiver => {
                         break;
                     },
                     message = recv_queue.next().fuse() => {
                        match message {
                            Some(Ok(m)) => backend.message_received(&peer_id, m),
                            Some(Err(e)) => {
                                log::debug!("Error receiving message: {:?}", e);
                                backend.peer_disconnected(&peer_id);
                                break;
                            }
                            None => {
                                backend.peer_disconnected(&peer_id);
                                break
                            }
                        }

                     }
                }
            }
        });
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        log::info!("Client disconnected {:?}", peer_id);
        if let Some(monitor) = self.monitor().lock().as_mut() {
            let _ = monitor.try_send(SocketEvent::Disconnected(peer_id.clone()));
        }
        self.connections.remove_sync(peer_id);
        let _ = self
            .fanout_events
            .unbounded_send(FanoutEvent::PeerDisconnected(peer_id.clone()));
    }
}

pub struct PubSocket {
    pub(crate) backend: Arc<PubSocketBackend>,
    fanout_state: FanoutState,
    fanout_events: FanoutEventReceiver,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for PubSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl SocketSend for PubSocket {
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

impl CaptureSocket for PubSocket {}

#[async_trait]
impl Socket for PubSocket {
    fn with_options(options: SocketOptions) -> Self {
        let (fanout_event_sender, fanout_events) = mpsc::unbounded();
        Self {
            backend: Arc::new(PubSocketBackend {
                connections: scc::HashMap::new(),
                fanout_events: fanout_event_sender,
                socket_monitor: Mutex::new(None),
                socket_options: options,
            }),
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
    use crate::util::tests::{
        test_bind_to_any_port_helper, test_bind_to_unspecified_interface_helper,
    };
    use crate::ZmqResult;
    use std::net::IpAddr;

    #[async_rt::test]
    async fn test_bind_to_any_port() -> ZmqResult<()> {
        let s = PubSocket::new();
        test_bind_to_any_port_helper(s).await
    }

    #[async_rt::test]
    async fn test_bind_to_any_ipv4_interface() -> ZmqResult<()> {
        let any_ipv4: IpAddr = "0.0.0.0".parse().unwrap();
        let s = PubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv4, s, 4000).await
    }

    #[async_rt::test]
    async fn test_bind_to_any_ipv6_interface() -> ZmqResult<()> {
        let any_ipv6: IpAddr = "::".parse().unwrap();
        let s = PubSocket::new();
        test_bind_to_unspecified_interface_helper(any_ipv6, s, 4010).await
    }
}
