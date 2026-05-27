#![recursion_limit = "1024"]

mod async_rt;
mod backend;
mod codec;
mod dealer;
mod endpoint;
mod error;
mod fair_queue;
mod message;
mod r#pub;
mod pull;
mod push;
mod reconnect;
mod rep;
mod req;
mod router;
mod sub;
mod sub_backend;
mod task_handle;
mod transport;
pub mod util;
mod write_queue;
mod xpub;
mod xsub;

#[doc(hidden)]
pub mod __async_rt {
    //! DO NOT USE! PRIVATE IMPLEMENTATION, EXPOSED ONLY FOR INTEGRATION TESTS.
    pub use super::async_rt::*;
}

#[cfg(feature = "bench-internals")]
#[doc(hidden)]
pub mod __bench {
    //! DO NOT USE! PRIVATE IMPLEMENTATION, EXPOSED ONLY FOR BENCHMARKS.
    pub use super::codec::{CodecError, Message, ZmqCodec, ZmqFramedRead};

    use super::backend::{GenericSocketBackend, Peer};
    use super::codec::ZmqFramedWrite;
    use super::r#pub::{PubSocketBackend, Subscriber};
    use super::util::PeerIdentity;
    use super::write_queue::write_message_queue;
    use super::{SocketOptions, SocketType, ZmqMessage, ZmqResult};
    use futures::channel::{mpsc, oneshot};
    use futures::AsyncWrite;
    use parking_lot::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    pub use super::fair_queue::FairQueue;

    pub struct BenchRoundRobinBackend {
        backend: GenericSocketBackend,
        receivers: Vec<mpsc::Receiver<Message>>,
    }

    impl BenchRoundRobinBackend {
        pub async fn new(peer_count: usize, queue_capacity: usize) -> Self {
            let backend = GenericSocketBackend::with_options(
                None,
                SocketType::PUSH,
                SocketOptions::default(),
            );
            let mut receivers = Vec::with_capacity(peer_count);

            for peer_index in 0..peer_count {
                let peer_id: PeerIdentity = format!("bench-peer-{peer_index}")
                    .parse()
                    .expect("valid peer identity");
                let (send_queue, recv_queue) = mpsc::channel(queue_capacity);
                backend
                    .peers
                    .upsert_async(peer_id.clone(), Peer { send_queue })
                    .await;
                backend.round_robin.push(peer_id);
                receivers.push(recv_queue);
            }

            Self { backend, receivers }
        }

        pub async fn send_round_robin(&self, message: Message) -> ZmqResult<()> {
            self.backend.send_round_robin(message).await
        }

        pub fn drain_ready(&mut self) -> usize {
            self.receivers
                .iter_mut()
                .filter_map(|receiver| receiver.try_recv().ok())
                .count()
        }
    }

    pub struct BenchPubFanoutBackend {
        backend: Arc<PubSocketBackend>,
        receivers: Vec<mpsc::Receiver<Message>>,
    }

    impl BenchPubFanoutBackend {
        pub async fn new(subscriber_count: usize, subscription: Vec<u8>) -> Self {
            Self::new_with_subscriptions(subscriber_count, vec![subscription]).await
        }

        pub async fn new_with_subscriptions(
            subscriber_count: usize,
            subscriptions: Vec<Vec<u8>>,
        ) -> Self {
            let backend = Arc::new(PubSocketBackend {
                subscribers: scc::HashMap::new(),
                subscriber_count: AtomicUsize::new(0),
                socket_monitor: Mutex::new(None),
                socket_options: SocketOptions::default(),
            });
            let mut receivers = Vec::with_capacity(subscriber_count);

            for subscriber_index in 0..subscriber_count {
                let peer_id: PeerIdentity = format!("bench-sub-{subscriber_index}")
                    .parse()
                    .expect("valid peer identity");
                let (send_queue, recv_queue) = mpsc::channel(1024);
                let (stop_sender, _stop_receiver) = oneshot::channel();
                backend
                    .subscribers
                    .upsert_async(
                        peer_id,
                        Subscriber {
                            subscriptions: subscriptions.clone(),
                            send_queue,
                            _subscription_coro_stop: stop_sender,
                        },
                    )
                    .await;
                backend
                    .subscriber_count
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                receivers.push(recv_queue);
            }

            Self { backend, receivers }
        }

        pub async fn fanout_message(&self, message: ZmqMessage) {
            self.backend.fanout_message(message).await;
        }

        pub fn drain_ready(&mut self) -> usize {
            self.receivers
                .iter_mut()
                .filter_map(|receiver| receiver.try_recv().ok())
                .count()
        }
    }

    pub fn zmq_framed_read<T>(inner: T) -> ZmqFramedRead
    where
        T: futures::AsyncRead + Unpin + Send + Sync + 'static,
    {
        ZmqFramedRead::new(Box::new(inner))
    }

    pub fn fair_queue_insert<S, K>(queue: &mut FairQueue<S, K>, key: K, stream: S)
    where
        K: Clone + Eq + std::hash::Hash,
    {
        queue.inner().lock().insert(key, stream);
    }

    pub fn fair_queue<S, K>(block_on_no_clients: bool) -> FairQueue<S, K>
    where
        K: Clone,
    {
        FairQueue::new(block_on_no_clients)
    }

    pub fn message_pop_front(message: &mut ZmqMessage) -> Option<bytes::Bytes> {
        message.pop_front()
    }

    pub async fn write_message_queue_to_writer<W>(
        receiver: mpsc::Receiver<Message>,
        writer: W,
    ) -> Result<(), CodecError>
    where
        W: AsyncWrite + Unpin + Send + Sync + 'static,
    {
        let send_queue = ZmqFramedWrite::new(Box::new(writer), ZmqCodec::new());
        write_message_queue(receiver, send_queue).await
    }
}

pub use crate::dealer::*;
pub use crate::endpoint::{Endpoint, Host, Transport, TryIntoEndpoint};
pub use crate::error::{ZmqError, ZmqResult};
pub use crate::message::*;
pub use crate::pull::*;
pub use crate::push::*;
pub use crate::r#pub::*;
pub use crate::rep::*;
pub use crate::req::*;
pub use crate::router::*;
pub use crate::sub::*;
pub use crate::xpub::*;
pub use crate::xsub::*;

use crate::codec::*;
use crate::transport::AcceptStopHandle;
use util::PeerIdentity;

use async_trait::async_trait;
use asynchronous_codec::FramedWrite;
use futures::channel::mpsc;
use futures::{select, FutureExt};
use parking_lot::Mutex;

use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::{Debug, Display};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

const COMPATIBILITY_MATRIX: [u8; 121] = [
    // PAIR, PUB, SUB, REQ, REP, DEALER, ROUTER, PULL, PUSH, XPUB, XSUB
    1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // PAIR
    0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, // PUB
    0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, // SUB
    0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, // REQ
    0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, // REP
    0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, // DEALER
    0, 0, 0, 1, 0, 1, 1, 0, 0, 0, 0, // ROUTER
    0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, // PULL
    0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, // PUSH
    0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, // XPUB
    0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, // XSUB
];

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(usize)]
pub enum SocketType {
    PAIR = 0,
    PUB = 1,
    SUB = 2,
    REQ = 3,
    REP = 4,
    DEALER = 5,
    ROUTER = 6,
    PULL = 7,
    PUSH = 8,
    XPUB = 9,
    XSUB = 10,
    STREAM = 11,
}

impl SocketType {
    pub const fn as_str(&self) -> &'static str {
        match self {
            SocketType::PAIR => "PAIR",
            SocketType::PUB => "PUB",
            SocketType::SUB => "SUB",
            SocketType::REQ => "REQ",
            SocketType::REP => "REP",
            SocketType::DEALER => "DEALER",
            SocketType::ROUTER => "ROUTER",
            SocketType::PULL => "PULL",
            SocketType::PUSH => "PUSH",
            SocketType::XPUB => "XPUB",
            SocketType::XSUB => "XSUB",
            SocketType::STREAM => "STREAM",
        }
    }

    /// Checks if two sockets are compatible with each other
    /// ```
    /// use zeromq::SocketType;
    /// assert!(SocketType::PUB.compatible(SocketType::SUB));
    /// assert!(SocketType::REQ.compatible(SocketType::REP));
    /// assert!(SocketType::DEALER.compatible(SocketType::ROUTER));
    /// assert!(!SocketType::PUB.compatible(SocketType::REP));
    /// ```
    pub fn compatible(&self, other: SocketType) -> bool {
        let row_index = *self as usize;
        let col_index = other as usize;
        COMPATIBILITY_MATRIX[row_index * 11 + col_index] != 0
    }
}

impl FromStr for SocketType {
    type Err = ZmqError;

    #[inline]
    fn from_str(s: &str) -> Result<Self, ZmqError> {
        Self::try_from(s.as_bytes())
    }
}

impl TryFrom<&[u8]> for SocketType {
    type Error = ZmqError;

    fn try_from(s: &[u8]) -> Result<Self, ZmqError> {
        Ok(match s {
            b"PAIR" => SocketType::PAIR,
            b"PUB" => SocketType::PUB,
            b"SUB" => SocketType::SUB,
            b"REQ" => SocketType::REQ,
            b"REP" => SocketType::REP,
            b"DEALER" => SocketType::DEALER,
            b"ROUTER" => SocketType::ROUTER,
            b"PULL" => SocketType::PULL,
            b"PUSH" => SocketType::PUSH,
            b"XPUB" => SocketType::XPUB,
            b"XSUB" => SocketType::XSUB,
            b"STREAM" => SocketType::STREAM,
            _ => return Err(ZmqError::Other("Unknown socket type")),
        })
    }
}

impl Display for SocketType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub enum SocketEvent {
    Connected(Endpoint, PeerIdentity),
    ConnectDelayed,
    ConnectRetried,
    Listening(Endpoint),
    Accepted(Endpoint, PeerIdentity),
    AcceptFailed(ZmqError),
    Closed,
    CloseFailed,
    Disconnected(PeerIdentity),
}

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub struct SocketOptions {
    pub(crate) peer_id: Option<PeerIdentity>,
    pub(crate) connect_timeout: Option<Duration>,
}

impl Default for SocketOptions {
    fn default() -> Self {
        Self {
            peer_id: None,
            connect_timeout: Some(DEFAULT_CONNECT_TIMEOUT),
        }
    }
}

impl SocketOptions {
    pub fn peer_identity(&mut self, peer_id: PeerIdentity) -> &mut Self {
        self.peer_id = Some(peer_id);
        self
    }

    pub fn connect_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.connect_timeout = Some(timeout);
        self
    }

    pub fn no_connect_timeout(&mut self) -> &mut Self {
        self.connect_timeout = None;
        self
    }
}

#[async_trait]
pub trait MultiPeerBackend: SocketBackend {
    /// This should not be public..
    /// Find a better way of doing this
    async fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo);

    fn peer_disconnected(&self, peer_id: &PeerIdentity);
}

pub trait SocketBackend: Send + Sync {
    fn socket_type(&self) -> SocketType;
    fn socket_options(&self) -> &SocketOptions;
    fn shutdown(&self);
    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>>;
}

#[async_trait]
pub trait SocketRecv {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage>;
}

#[async_trait]
pub trait SocketSend {
    /// Sends one message according to the socket type's send policy.
    ///
    /// For sockets backed by internal writer queues, `Ok(())` means the
    /// message has entered the local send path. It does not mean the message
    /// has been flushed to the transport, received by the peer, or processed by
    /// the peer.
    ///
    /// PUB sockets may drop messages according to PUB slow-subscriber policy.
    /// Non-PUB queued backends still apply backpressure when the selected peer
    /// queue is full.
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()>;
}

/// Marker trait that express the fact that only certain types of sockets might be used
/// in [proxy] function as a capture parameter
pub trait CaptureSocket: SocketSend {}

#[allow(clippy::empty_line_after_outer_attr)]
#[async_trait]
pub trait Socket: Sized + Send {
    fn new() -> Self {
        Self::with_options(SocketOptions::default())
    }

    fn with_options(options: SocketOptions) -> Self;

    fn backend(&self) -> Arc<dyn MultiPeerBackend>;

    /// Binds to the endpoint and starts a coroutine to accept new connections
    /// on it.
    ///
    /// Returns the endpoint resolved to the exact bound location if applicable
    /// (port # resolved, for example).
    async fn bind(&mut self, endpoint: &str) -> ZmqResult<Endpoint> {
        let endpoint = TryIntoEndpoint::try_into(endpoint)?;

        let cloned_backend = self.backend();
        let cback = move |result: ZmqResult<(FramedIo, Endpoint)>| {
            let cloned_backend = cloned_backend.clone();
            async move {
                let result = match result {
                    Ok((socket, endpoint)) => util::peer_connected(socket, cloned_backend.clone())
                        .await
                        .map(|peer_id| (endpoint, peer_id)),
                    Err(e) => Err(e),
                };

                match result {
                    Ok((endpoint, peer_id)) => {
                        if let Some(monitor) = cloned_backend.monitor().lock().as_mut() {
                            let _ = monitor.try_send(SocketEvent::Accepted(endpoint, peer_id));
                        }
                    }
                    Err(e) => {
                        if let Some(monitor) = cloned_backend.monitor().lock().as_mut() {
                            let _ = monitor.try_send(SocketEvent::AcceptFailed(e));
                        }
                    }
                }
            }
        };

        let (endpoint, stop_handle) = transport::begin_accept(endpoint, cback).await?;

        if let Some(monitor) = self.backend().monitor().lock().as_mut() {
            let _ = monitor.try_send(SocketEvent::Listening(endpoint.clone()));
        }

        self.binds().insert(endpoint.clone(), stop_handle);
        Ok(endpoint)
    }

    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle>;

    /// Unbinds the endpoint, blocking until the associated endpoint is no
    /// longer in use
    ///
    /// # Errors
    /// May give a `ZmqError::NoSuchBind` if `endpoint` isn't bound. May also
    /// give any other zmq errors encountered when attempting to disconnect
    async fn unbind(&mut self, endpoint: Endpoint) -> ZmqResult<()> {
        let stop_handle = self.binds().remove(&endpoint);
        let stop_handle = stop_handle.ok_or(ZmqError::NoSuchBind(endpoint))?;
        stop_handle.0.shutdown().await
    }

    /// Unbinds all bound endpoints, blocking until finished.
    async fn unbind_all(&mut self) -> Vec<ZmqError> {
        let mut errs = Vec::new();
        let endpoints: Vec<_> = self.binds().keys().cloned().collect();
        for endpoint in endpoints {
            if let Err(err) = self.unbind(endpoint).await {
                errs.push(err);
            }
        }
        errs
    }

    /// Connects to the given endpoint.
    async fn connect(&mut self, endpoint: &str) -> ZmqResult<()> {
        let backend = self.backend();
        let endpoint = TryIntoEndpoint::try_into(endpoint)?;
        let connect_timeout = backend.socket_options().connect_timeout;

        let (socket, endpoint, peer_id) = util::run_with_timeout(connect_timeout, async {
            let (mut socket, endpoint) = util::connect_forever(endpoint).await?;
            let peer_id = util::peer_handshake(&mut socket, backend.clone()).await?;
            Ok((socket, endpoint, peer_id))
        })
        .await?;
        backend.peer_connected(&peer_id, socket).await;

        if let Some(monitor) = self.backend().monitor().lock().as_mut() {
            let _ = monitor.try_send(SocketEvent::Connected(endpoint, peer_id));
        }
        Ok(())
    }

    /// Creates and setups new socket monitor
    ///
    /// Subsequent calls to this method each create a new monitor channel.
    /// Sender side of previous one is dropped.
    fn monitor(&mut self) -> mpsc::Receiver<SocketEvent>;

    // TODO: async fn connections(&self) -> ?

    /// Disconnects from the given endpoint, blocking until finished.
    ///
    /// # Errors
    /// May give a `ZmqError::NoSuchConnection` if `endpoint` isn't connected.
    /// May also give any other zmq errors encountered when attempting to
    /// disconnect
    // TODO: async fn disconnect(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) ->
    // ZmqResult<()>;

    /// Disconnects all connections, blocking until finished.
    // TODO: async fn disconnect_all(&mut self) -> ZmqResult<()>;

    /// Closes the socket, blocking until all associated binds are closed.
    /// This is equivalent to `drop()`, but with the benefit of blocking until
    /// resources are released, and getting any underlying errors.
    ///
    /// Returns any encountered errors.
    // TODO: Call disconnect_all() when added
    async fn close(mut self) -> Vec<ZmqError> {
        // self.disconnect_all().await?;
        self.unbind_all().await
    }
}

pub async fn proxy<Frontend: SocketSend + SocketRecv, Backend: SocketSend + SocketRecv>(
    mut frontend: Frontend,
    mut backend: Backend,
    mut capture: Option<Box<dyn CaptureSocket>>,
) -> ZmqResult<()> {
    loop {
        select! {
            frontend_mess = frontend.recv().fuse() => {
                match frontend_mess {
                    Ok(message) => {
                        if let Some(capture) = &mut capture {
                            capture.send(message.clone()).await?;
                        }
                        backend.send(message).await?;
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            },
            backend_mess = backend.recv().fuse() => {
                match backend_mess {
                    Ok(message) => {
                        if let Some(capture) = &mut capture {
                            capture.send(message.clone()).await?;
                        }
                        frontend.send(message).await?;
                    }
                    Err(e) => {
                        return Err(e);
                    }
                }
            }
        };
    }
}

pub mod prelude {
    //! Re-exports important traits. Consider glob-importing.

    pub use crate::{Socket, SocketRecv, SocketSend, TryIntoEndpoint};
}
