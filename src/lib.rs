#[macro_use]
extern crate enum_primitive_derive;
use num_traits::{FromPrimitive, ToPrimitive};

use bytes::{Buf, Bytes, BytesMut};
use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::stream::StreamExt;
use futures_util::sink::SinkExt;
use tokio::prelude::*;
use tokio_util::codec::Framed;

use std::convert::TryFrom;
use std::fmt::Display;

mod codec;
mod error;
mod req;

#[cfg(test)]
mod tests;

use crate::codec::*;
use crate::error::ZmqError;
use crate::SocketType::REQ;
use crate::req::ReqSocket;

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

/// Checks if two sokets are compatible with each other
/// ```
/// use zmq_rs::{sockets_compatible, SocketType};
/// assert!(sockets_compatible(SocketType::PUB, SocketType::SUB));
/// assert!(sockets_compatible(SocketType::REQ, SocketType::REP));
/// assert!(sockets_compatible(SocketType::DEALER, SocketType::ROUTER));
/// ```
pub fn sockets_compatible(one: SocketType, another: SocketType) -> bool {
    let row_index = one.to_usize().unwrap();
    let col_index = another.to_usize().unwrap();
    COMPATIBILITY_MATRIX[row_index * 11 + col_index] != 0
}


#[async_trait]
pub trait Socket {
    async fn send(&mut self, data: ZmqMessage) -> ZmqResult<()>;
    async fn recv(&mut self) -> ZmqResult<ZmqMessage>;
}

pub async fn connect(socket_type: SocketType, endpoint: &str) -> ZmqResult<Box<dyn Socket>> {
    let addr = endpoint.parse::<SocketAddr>()?;
    let mut raw_socket = Framed::new(TcpStream::connect(addr).await?, ZmqCodec::new());
    raw_socket.send(Message::Greeting(ZmqGreeting::default())).await?;

    let greeting: Option<Result<Message, ZmqError>> = raw_socket.next().await;

    match greeting {
        Some(Ok(Message::Greeting(greet))) => match greet.version {
            (3, 0) => {},
            _ => return Err(ZmqError::OTHER("Unsupported protocol version"))
        },
        _ => return Err(ZmqError::CODEC("Failed Greeting exchange"))
    };

    let ready = ZmqCommand::ready(SocketType::REQ);
    raw_socket.send(Message::Command(ready)).await?;

    let ready_repl = raw_socket.next().await;
    dbg!(&ready_repl);

    let socket = match socket_type {
        SocketType::REQ => Box::new(ReqSocket { _inner: raw_socket }),
        _ => return Err(ZmqError::OTHER("Socket type not supported")),
    };
    Ok(socket)
}
