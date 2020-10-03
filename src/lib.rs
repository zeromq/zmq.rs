#![recursion_limit = "1024"]
#[macro_use]
extern crate enum_primitive_derive;
use num_traits::ToPrimitive;

use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};
use std::convert::TryFrom;
use std::fmt::{Debug, Display};

use tokio_util::codec::Framed;

mod codec;
mod dealer_router;
mod endpoint;
mod error;
mod fair_queue;
mod message;
mod r#pub;
mod rep;
mod req;
mod sub;
pub mod util;

use crate::codec::*;
pub use crate::dealer_router::*;
pub use crate::endpoint::{Endpoint, Host, Transport, TryIntoEndpoint};
pub use crate::error::ZmqError;
pub use crate::r#pub::*;
pub use crate::rep::*;
pub use crate::req::*;
pub use crate::sub::*;
use crate::util::*;
pub use message::*;

pub type ZmqResult<T> = Result<T, ZmqError>;

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
            _ => return Err(ZmqError::Codec("Unknown socket type")),
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

#[async_trait]
trait MultiPeer: SocketBackend {
    async fn peer_connected(
        &self,
        peer_id: &PeerIdentity,
    ) -> (mpsc::Receiver<Message>, oneshot::Receiver<bool>);
    async fn peer_disconnected(&self, peer_id: &PeerIdentity);
}

#[async_trait]
trait SocketBackend: Send + Sync {
    async fn message_received(&self, peer_id: &PeerIdentity, message: Message);

    fn socket_type(&self) -> SocketType;
    fn shutdown(&self);
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
pub trait Socket {
    fn new() -> Self;

    /// Opens port described by endpoint and starts a coroutine to accept new
    /// connections on it.
    ///
    /// Returns the bound-to endpoint, with the port resolved (if applicable).
    async fn bind(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait)
        -> ZmqResult<&Endpoint>;

    // TODO: Although it would reduce how convenient the function is, taking an
    // `&Endpoint` would be better for performance here
    async fn connect(&mut self, endpoint: impl TryIntoEndpoint + 'async_trait) -> ZmqResult<()>;

    /// Retrieves the list of currently bound endpoints
    fn binds(&self) -> &[Endpoint];
}

pub mod prelude {
    //! Re-exports important traits. Consider glob-importing.

    pub use crate::{
        BlockingRecv, BlockingSend, NonBlockingRecv, NonBlockingSend, Socket, TryIntoEndpoint,
    };
}
