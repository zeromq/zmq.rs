#[macro_use]
extern crate enum_primitive_derive;
use num_traits::{FromPrimitive, ToPrimitive};

use async_trait::async_trait;
use futures_util::sink::SinkExt;
use tokio::net::TcpListener;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::stream::StreamExt;
use tokio_util::codec::Framed;

use std::convert::TryFrom;
use std::fmt::{Display, Debug};

mod codec;
mod error;
mod req;
mod rep;
mod sub;
mod util;

#[cfg(test)]
mod tests;

use crate::codec::*;
use crate::error::ZmqError;
use crate::req::*;
use crate::rep::*;
use crate::sub::*;
use crate::util::*;

pub use crate::codec::ZmqMessage;

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
            _ => return Err(ZmqError::CODEC("Unknown socket type")),
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
pub trait Socket: Send {
    async fn send(&mut self, data: Vec<u8>) -> ZmqResult<()>;
    async fn recv(&mut self) -> ZmqResult<Vec<u8>>;
}

#[async_trait]
pub trait SocketServer {
    async fn accept(&mut self) -> ZmqResult<Box<dyn Socket>>;
}

pub async fn connect(socket_type: SocketType, endpoint: &str) -> ZmqResult<Box<dyn Socket>> {
    let addr = endpoint.parse::<SocketAddr>()?;
    let mut raw_socket = Framed::new(TcpStream::connect(addr).await?, ZmqCodec::new());
    greet_exchange(&mut raw_socket).await?;

    ready_exchange(&mut raw_socket, socket_type).await?;


    let socket: Box<dyn Socket> = match socket_type {
        SocketType::REQ => Box::new(ReqSocket { _inner: raw_socket }),
        SocketType::SUB => {
            let mut sock = SubSocket { _inner: raw_socket };
            sock.subscribe("").await?;
            Box::new(sock)
        },
        _ => return Err(ZmqError::OTHER("Socket type not supported")),
    };
    Ok(socket)
}

pub async fn bind(socket_type: SocketType, endpoint: &str) -> ZmqResult<Box<dyn SocketServer>> {
    let listener = TcpListener::bind(endpoint).await?;
    match socket_type {
        SocketType::REP => Ok(Box::new(RepSocketServer { _inner: listener })),
        _ => todo!()
    }
}
