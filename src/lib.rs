#![recursion_limit = "1024"]

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
mod rep;
mod req;
mod router;
mod sub;
mod task_handle;
mod transport;
pub mod util;

pub use crate::dealer::*;
pub use crate::endpoint::{Endpoint, Host, Transport, TryIntoEndpoint};
pub use crate::error::{ZmqError, ZmqResult};
pub use crate::pull::*;
pub use crate::push::*;
pub use crate::r#pub::*;
pub use crate::rep::*;
pub use crate::req::*;
pub use crate::router::*;
pub use crate::sub::*;
pub use message::*;

use crate::codec::*;
use crate::transport::AcceptStopHandle;
use util::PeerIdentity;

#[macro_use]
extern crate enum_primitive_derive;

use async_trait::async_trait;
use futures::channel::mpsc;
use futures_codec::FramedWrite;
use num_traits::ToPrimitive;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::{Debug, Display};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Primitive)]
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

impl TryFrom<&str> for SocketType {
    type Error = ZmqError;

    fn try_from(s: &str) -> Result<Self, ZmqError> {
        Ok(match s {
            "PAIR" => SocketType::PAIR,
            "PUB" => SocketType::PUB,
            "SUB" => SocketType::SUB,
            "REQ" => SocketType::REQ,
            "REP" => SocketType::REP,
            "DEALER" => SocketType::DEALER,
            "ROUTER" => SocketType::ROUTER,
            "PULL" => SocketType::PULL,
            "PUSH" => SocketType::PUSH,
            "XPUB" => SocketType::XPUB,
            "XSUB" => SocketType::XSUB,
            "STREAM" => SocketType::STREAM,
            _ => return Err(ZmqError::Other("Unknown socket type")),
        })
    }
}

impl Display for SocketType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SocketType::PAIR => write!(f, "PAIR"),
            SocketType::PUB => write!(f, "PUB"),
            SocketType::SUB => write!(f, "SUB"),
            SocketType::REQ => write!(f, "REQ"),
            SocketType::REP => write!(f, "REP"),
            SocketType::DEALER => write!(f, "DEALER"),
            SocketType::ROUTER => write!(f, "ROUTER"),
            SocketType::PULL => write!(f, "PULL"),
            SocketType::PUSH => write!(f, "PUSH"),
            SocketType::XPUB => write!(f, "XPUB"),
            SocketType::XSUB => write!(f, "XSUB"),
            SocketType::STREAM => write!(f, "STREAM"),
        }
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

pub trait MultiPeerBackend: SocketBackend {
    /// This should not be public..
    /// Find a better way of doing this

    fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo);
    fn peer_disconnected(&self, peer_id: &PeerIdentity);
}

pub trait SocketBackend: Send + Sync {
    fn socket_type(&self) -> SocketType;
    fn shutdown(&self);
    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>>;
}

#[async_trait]
pub trait BlockingRecv {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage>;
}

#[async_trait]
pub trait BlockingSend {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()>;
}

pub trait NonBlockingSend {
    fn send(&mut self, message: ZmqMessage) -> ZmqResult<()>;
}

pub trait NonBlockingRecv {
    fn recv(&mut self) -> ZmqResult<ZmqMessage>;
}

#[async_trait]
pub trait Socket: Sized + Send {
    fn new() -> Self;

    fn backend(&self) -> Arc<dyn MultiPeerBackend>;

    /// Binds to the endpoint and starts a coroutine to accept new connections
    /// on it.
    ///
    /// Returns the endpoint resolved to the exact bound location if applicable
    /// (port # resolved, for example).
    async fn bind(&mut self, endpoint: &str) -> ZmqResult<Endpoint> {
        let endpoint = endpoint.try_into()?;

        let cloned_backend = self.backend();
        let cback = move |result| {
            let cloned_backend = cloned_backend.clone();
            async move {
                let result = match result {
                    Ok((socket, endpoint)) => {
                        match util::peer_connected(socket, cloned_backend.clone()).await {
                            Ok(peer_id) => Ok((endpoint, peer_id)),
                            Err(e) => Err(e),
                        }
                    }
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
        let endpoints: Vec<_> = self
            .binds()
            .iter()
            .map(|(endpoint, _)| endpoint.clone())
            .collect();
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
        let endpoint = endpoint.try_into()?;

        let result = match transport::connect(endpoint).await {
            Ok((socket, endpoint)) => match util::peer_connected(socket, backend).await {
                Ok(peer_id) => Ok((endpoint, peer_id)),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        match result {
            Ok((endpoint, peer_id)) => {
                if let Some(monitor) = self.backend().monitor().lock().as_mut() {
                    let _ = monitor.try_send(SocketEvent::Connected(endpoint, peer_id));
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
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
    /// disconnect.
    // TODO: async fn disconnect(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) ->
    // ZmqResult<()>;

    /// Disconnects all connecttions, blocking until finished.
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

pub mod prelude {
    //! Re-exports important traits. Consider glob-importing.

    pub use crate::{
        BlockingRecv, BlockingSend, NonBlockingRecv, NonBlockingSend, Socket, TryIntoEndpoint,
    };
}
