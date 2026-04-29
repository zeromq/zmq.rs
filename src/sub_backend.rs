use crate::backend::DisconnectNotifier;
use crate::codec::{CodecError, FramedIo, Message, ZmqFramedRead, ZmqFramedWrite};
use crate::endpoint::Endpoint;
use crate::error::ZmqResult;
use crate::fair_queue::QueueInner;
use crate::message::ZmqMessage;
use crate::reconnect::{ReconnectConfig, ReconnectHandle};
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{
    MultiPeerBackend, SocketBackend, SocketEvent, SocketOptions, SocketType, TryIntoEndpoint,
};

use async_trait::async_trait;
use bytes::{BufMut, BytesMut};
use futures::channel::mpsc;
use futures::lock::Mutex as AsyncMutex;
use futures::SinkExt;
use parking_lot::Mutex;

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::pin::Pin;
use std::sync::Arc;

/// Type of subscription message sent from SUB/XSUB to PUB/XPUB.
/// The discriminant values match the ZMTP wire format.
pub(crate) enum SubscriptionMessageType {
    Unsubscribe = 0,
    Subscribe = 1,
}

/// A connected peer for SUB/XSUB sockets, holding only the send half
/// of the framed I/O since receiving is handled through the fair queue.
pub(crate) struct SubPeer {
    pub(crate) send_queue: Arc<AsyncMutex<Pin<Box<ZmqFramedWrite>>>>,
}

/// Shared backend for [`SubSocket`](crate::SubSocket) and [`XSubSocket`](crate::XSubSocket).
///
/// Manages peer connections, subscription state, and disconnect notifications.
/// When a new peer connects, all current subscriptions are automatically
/// re-sent to ensure the peer receives the full subscription set.
pub(crate) struct SubSocketBackend {
    pub(crate) peers: scc::HashMap<PeerIdentity, SubPeer>,
    fair_queue_inner: Option<Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>>,
    socket_type: SocketType,
    socket_options: SocketOptions,
    pub(crate) socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
    subs: Mutex<HashSet<Vec<u8>>>,
    /// Notifiers for reconnection tasks - keyed by `peer_id`
    disconnect_notifiers: Mutex<HashMap<PeerIdentity, DisconnectNotifier>>,
}

impl SubSocketBackend {
    pub(crate) fn with_options(
        fair_queue_inner: Option<Arc<Mutex<QueueInner<ZmqFramedRead, PeerIdentity>>>>,
        socket_type: SocketType,
        options: SocketOptions,
    ) -> Self {
        Self {
            peers: scc::HashMap::new(),
            fair_queue_inner,
            socket_type,
            socket_options: options,
            socket_monitor: Mutex::new(None),
            subs: Mutex::new(HashSet::new()),
            disconnect_notifiers: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn create_subs_message(
        subscription: &[u8],
        msg_type: SubscriptionMessageType,
    ) -> ZmqMessage {
        let mut buf = BytesMut::with_capacity(subscription.len() + 1);
        buf.put_u8(msg_type as u8);
        buf.extend_from_slice(subscription);
        buf.freeze().into()
    }

    /// Track a subscription locally so it can be re-sent on reconnection.
    pub(crate) fn remember_subscription(&self, subscription: &[u8]) {
        self.subs.lock().insert(subscription.to_vec());
    }

    /// Remove a subscription from local tracking.
    pub(crate) fn forget_subscription(&self, subscription: &[u8]) {
        self.subs.lock().remove(subscription);
    }

    /// Parse a ZMTP subscription message and update local subscription state.
    ///
    /// Returns `true` if the message was a valid subscription/unsubscription
    /// frame (single frame, first byte 0x01 for subscribe or 0x00 for
    /// unsubscribe, remainder is the subscription prefix).
    /// Returns `false` for any other message, which should be treated as
    /// an application-level message.
    pub(crate) fn apply_subscription_message(&self, message: &ZmqMessage) -> bool {
        let Some(frame) = message.get(0) else {
            return false;
        };
        if message.len() != 1 || frame.is_empty() {
            return false;
        }

        match frame[0] {
            1 => {
                self.remember_subscription(&frame[1..]);
                true
            }
            0 => {
                self.forget_subscription(&frame[1..]);
                true
            }
            _ => false,
        }
    }

    /// Register a notifier to be called when a peer disconnects.
    ///
    /// Used by reconnection tasks to be notified when they should attempt reconnection.
    pub(crate) fn register_disconnect_notifier(
        &self,
        peer_id: PeerIdentity,
        notifier: DisconnectNotifier,
    ) {
        self.disconnect_notifiers.lock().insert(peer_id, notifier);
    }

    /// Build and reliably send a subscription/unsubscription message to all peers.
    pub(crate) async fn broadcast_subscription(
        &self,
        subscription: &[u8],
        msg_type: SubscriptionMessageType,
    ) -> ZmqResult<()> {
        let message = Self::create_subs_message(subscription, msg_type);
        self.broadcast_message_reliably(message).await
    }

    /// Send a control message to all connected peers, awaiting each send.
    ///
    /// Unlike [`fanout_message`](Self::fanout_message), this uses async send
    /// which applies backpressure and returns errors on failure — suitable for
    /// subscription messages that must be delivered reliably.
    pub(crate) async fn broadcast_message_reliably(&self, message: ZmqMessage) -> ZmqResult<()> {
        self.broadcast_control_message(message).await
    }

    /// Send an application message to all connected peers.
    ///
    /// Peers with broken pipes are collected and disconnected after the
    /// iteration completes.
    pub(crate) async fn fanout_message(&self, message: ZmqMessage) -> ZmqResult<()> {
        if message.is_empty() {
            return Ok(());
        }

        let mut targets = Vec::new();
        let mut iter = self.peers.begin_async().await;
        while let Some(peer) = iter {
            targets.push((peer.key().clone(), peer.send_queue.clone()));
            iter = peer.next_async().await;
        }

        let mut dead_peers = Vec::new();
        for (peer_id, send_queue) in targets {
            let res = send_queue
                .lock()
                .await
                .as_mut()
                .send(Message::Message(message.clone()))
                .await;
            match res {
                Ok(()) => {}
                Err(CodecError::Io(e)) => {
                    if e.kind() == ErrorKind::BrokenPipe {
                        dead_peers.push(peer_id);
                    } else {
                        log::error!("Error sending message: {:?}", e);
                    }
                }
                Err(e) => {
                    log::error!("Error sending message: {:?}", e);
                    return Err(e.into());
                }
            }
        }

        for peer_id in dead_peers {
            self.peer_disconnected(&peer_id);
        }

        Ok(())
    }

    /// Collect all current subscriptions as ZMTP subscribe messages,
    /// ready to be sent to a newly connected peer.
    fn subscription_messages(&self) -> Vec<ZmqMessage> {
        self.subs
            .lock()
            .iter()
            .map(|subscription| {
                Self::create_subs_message(subscription, SubscriptionMessageType::Subscribe)
            })
            .collect()
    }

    /// Reliably send a message to every connected peer using async send.
    async fn broadcast_control_message(&self, message: ZmqMessage) -> ZmqResult<()> {
        let mut targets = Vec::new();
        let mut iter = self.peers.begin_async().await;
        while let Some(peer) = iter {
            targets.push((peer.key().clone(), peer.send_queue.clone()));
            iter = peer.next_async().await;
        }

        let mut dead_peers = Vec::new();
        let mut first_error = None;
        for (peer_id, send_queue) in targets {
            let result = send_queue
                .lock()
                .await
                .as_mut()
                .send(Message::Message(message.clone()))
                .await;
            match result {
                Ok(()) => {}
                Err(e)
                    if matches!(
                        &e,
                        CodecError::Io(io_error) if io_error.kind() == ErrorKind::BrokenPipe
                    ) =>
                {
                    dead_peers.push(peer_id);
                    first_error.get_or_insert(e.into());
                }
                Err(e) => {
                    log::error!(
                        "Error sending control message to peer {:?}: {:?}",
                        peer_id,
                        e
                    );
                    first_error.get_or_insert(e.into());
                }
            }
        }

        for peer_id in dead_peers {
            self.peer_disconnected(&peer_id);
        }

        first_error.map_or(Ok(()), Err)
    }
}

impl SocketBackend for SubSocketBackend {
    fn socket_type(&self) -> SocketType {
        self.socket_type
    }

    fn socket_options(&self) -> &SocketOptions {
        &self.socket_options
    }

    fn shutdown(&self) {
        self.peers.clear_sync();
        // Clear fair_queue streams to ensure TCP connections are closed
        // even when reconnect tasks still hold Arc references to the backend
        if let Some(inner) = &self.fair_queue_inner {
            inner.lock().clear();
        }
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

#[async_trait]
impl MultiPeerBackend for SubSocketBackend {
    async fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        let (recv_queue, mut send_queue) = io.into_parts();

        for message in self.subscription_messages() {
            if let Err(e) = send_queue.send(Message::Message(message)).await {
                log::error!("Failed to send subscription to peer {:?}: {:?}", peer_id, e);
                return;
            }
        }

        self.peers
            .upsert_async(
                peer_id.clone(),
                SubPeer {
                    send_queue: Arc::new(AsyncMutex::new(Box::pin(send_queue))),
                },
            )
            .await;

        if let Some(inner) = &self.fair_queue_inner {
            inner.lock().insert(peer_id.clone(), recv_queue);
        }
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        if let Some(monitor) = self.monitor().lock().as_mut() {
            let _ = monitor.try_send(SocketEvent::Disconnected(peer_id.clone()));
        }

        self.peers.remove_sync(peer_id);
        if let Some(inner) = &self.fair_queue_inner {
            inner.lock().remove(peer_id);
        }

        // Notify reconnection task if registered
        if let Some(mut notifier) = self.disconnect_notifiers.lock().remove(peer_id) {
            // Use try_send to avoid blocking - if channel is full, the reconnect task
            // will eventually notice the peer is gone
            let _ = notifier.try_send(peer_id.clone());
        }
    }
}

/// Connects to the given endpoint with automatic reconnection support.
///
/// Performs the initial connection, then spawns a background task that will
/// automatically reconnect if the connection is lost. On reconnection,
/// subscriptions are automatically re-sent to the peer.
pub(crate) async fn connect_with_reconnect(
    backend: Arc<SubSocketBackend>,
    reconnect_handles: &mut Vec<ReconnectHandle>,
    endpoint: &str,
) -> ZmqResult<()> {
    let endpoint = TryIntoEndpoint::try_into(endpoint)?;
    let connect_timeout = backend.socket_options().connect_timeout;

    // Initial connection
    let (socket, resolved_endpoint, peer_id) =
        crate::util::run_with_timeout(connect_timeout, async {
            let (mut socket, resolved_endpoint) =
                crate::util::connect_forever(endpoint.clone()).await?;
            let peer_id = crate::util::peer_handshake(
                &mut socket,
                backend.clone() as Arc<dyn MultiPeerBackend>,
            )
            .await?;
            Ok((socket, resolved_endpoint, peer_id))
        })
        .await?;
    backend.clone().peer_connected(&peer_id, socket).await;

    // Emit Connected event
    if let Some(monitor) = backend.monitor().lock().as_mut() {
        let _ = monitor.try_send(SocketEvent::Connected(resolved_endpoint, peer_id.clone()));
    }

    // Create a closure that registers disconnect notifiers with the backend
    let backend_for_closure = backend.clone();
    let register_fn: crate::reconnect::RegisterDisconnectFn = Box::new(move |peer_id, notifier| {
        backend_for_closure.register_disconnect_notifier(peer_id, notifier);
    });

    // Spawn reconnection task
    let reconnect_handle = crate::reconnect::spawn_reconnect_task(
        endpoint,
        backend as Arc<dyn MultiPeerBackend>,
        peer_id,
        register_fn,
        ReconnectConfig::default(),
    );
    reconnect_handles.push(reconnect_handle);

    Ok(())
}

/// Type alias for the bind map shared by SUB and XSUB sockets.
pub(crate) type SocketBinds = HashMap<Endpoint, AcceptStopHandle>;
