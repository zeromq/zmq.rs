//! An asynchronous ZMQ library for Rust.
//!
//! # Examples
//!
//! ```
//! use std::convert::TryInto;
//! use std::error::Error;
//! use zeromq::ZmqMessage;
//! use zeromq::{Socket, SocketType};
//! 
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn Error>> {
//!     let mut socket = zeromq::ReqSocket::connect("127.0.0.1:5555")
//!         .await
//!         .expect("Failed to connect");
//! 
//!     socket.send("Hello".into()).await?;
//!     let repl: String = socket.recv().await?.try_into()?;
//!     dbg!(repl);
//! 
//!     socket.send("NewHello".into()).await?;
//!     let repl: String = socket.recv().await?.try_into()?;
//!     dbg!(repl);
//!     Ok(())
//! }
//! ```

#![recursion_limit = "1024"]
#![deny(missing_docs)]

#[macro_use]
extern crate enum_primitive_derive;
use num_traits::ToPrimitive;

use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_util::codec::Framed;
use std::convert::TryFrom;
use std::fmt::{Debug, Display};

mod codec;
mod dealer_router;
mod error;
mod message;
mod pub_sub;
mod req_rep;
pub mod util;

#[cfg(test)]
mod tests;

use crate::codec::*;
pub use crate::dealer_router::*;
pub use crate::error::ZmqError;
pub use crate::pub_sub::*;
pub use crate::req_rep::*;
use crate::util::*;
pub use message::*;

/// The library result type.
pub type ZmqResult<T> = Result<T, ZmqError>;

/// An enum of all various ZMQ socket types.
#[derive(Clone, Copy, Debug, PartialEq, Primitive)]
pub enum SocketType {
    /// A ZMQ `PAIR` type socket.
    PAIR = 0,

    /// A ZMQ `PUB` type socket.
    PUB = 1,

    /// A ZMQ `SUB` type socket.
    SUB = 2,

    /// A ZMQ `REQ` type socket.
    REQ = 3,

    /// A ZMQ `REP` type socket.
    REP = 4,

    /// A ZMQ `DEALER` type socket.
    DEALER = 5,

    /// A ZMQ `ROUTER` type socket.
    ROUTER = 6,

    /// A ZMQ `PULL` type socket.
    PULL = 7,

    /// A ZMQ `PUSH` type socket.
    PUSH = 8,

    /// A ZMQ `XPUB` type socket.
    XPUB = 9,

    /// A ZMQ `XSUB` type socket.
    XSUB = 10,

    /// A ZMQ `STREAM` type socket.
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

trait MultiPeer: SocketBackend {
    fn peer_connected(
        &self,
        peer_id: &PeerIdentity,
    ) -> (mpsc::Receiver<Message>, oneshot::Receiver<bool>);
    fn peer_disconnected(&self, peer_id: &PeerIdentity);
}

#[async_trait]
trait SocketBackend: Send + Sync {
    async fn message_received(&self, peer_id: &PeerIdentity, message: Message);

    fn socket_type(&self) -> SocketType;
    fn shutdown(&self);
}

/// A trait to represent socket types.
#[async_trait]
pub trait Socket: Send {
    /// Send a message through the socket.
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()>;

    /// Receive a message through the socket.
    async fn recv(&mut self) -> ZmqResult<ZmqMessage>;
}


/// A frontend for socket types to implement.
#[async_trait]
pub trait SocketFrontend {
    /// A trait level constructor.
    fn new() -> Self;

    /// Opens port described by endpoint and starts a coroutine to accept new connections on it
    async fn bind(&mut self, endpoint: &str) -> ZmqResult<()>;

    /// Connect to another socket on some endpoint.
    async fn connect(&mut self, endpoint: &str) -> ZmqResult<()>;
}

/// A trait for all bound sockets, acting like servers.
#[async_trait]
pub trait SocketServer {
    /// Accept a connection.
    async fn accept(&mut self) -> ZmqResult<Box<dyn Socket>>;
}

/// Create a bound socket of some type.
pub async fn bind(socket_type: SocketType, endpoint: &str) -> ZmqResult<Box<dyn SocketServer>> {
    let listener = TcpListener::bind(endpoint).await?;
    match socket_type {
        SocketType::REP => Ok(Box::new(RepSocketServer { _inner: listener })),
        _ => todo!(),
    }
}

/// Not implemented.
pub async fn proxy(_s1: Box<dyn Socket>, _s2: Box<dyn Socket>) -> ZmqResult<()> {
    todo!()
}
