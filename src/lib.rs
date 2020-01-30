use std::error::Error;

#[macro_use]
extern crate enum_primitive_derive;
use num_traits::{FromPrimitive, ToPrimitive};

use bytes::{BytesMut, Buf, Bytes};
use std::net::{SocketAddr, AddrParseError};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::{ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::stream::StreamExt;
use futures_util::sink::SinkExt;
use tokio_util::codec::Framed;
use tokio_util::codec::{Decoder, Encoder};

use std::fmt::Display;
use crate::ZmqError::{NETWORK, NO_MESSAGE};
use crate::Message::Greeting;
use bytes::buf::BufExt;
use std::convert::TryFrom;
use crate::error::ZmqError;

mod error;
mod codec;

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

#[derive(Debug, Copy, Clone)]
pub struct ZmqCommand {

}

#[derive(Debug, Copy, Clone)]
pub enum ZmqMechanism {
    NULL,
    PLAIN,
    CURVE
}

#[derive(Debug, Copy, Clone)]
pub struct ZmqGreeting {
    pub version: (u8, u8),
    pub mechanism: ZmqMechanism,
    pub asServer: bool
}

impl TryFrom<Bytes> for ZmqGreeting {
    type Error = ZmqError;

    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        if !(value[0] == 0xff && value[9] == 0x7f) {
            return Err(ZmqError::CODEC("Failed to parse greeting"))
        }
        Ok(ZmqGreeting {
            version: (value[10], value[11]),
            mechanism: ZmqMechanism::NULL,
            asServer: value[32] == 0x01
        })
    }
}

impl From<ZmqGreeting> for BytesMut {

    fn from(greet: ZmqGreeting) -> Self {
        let mut data: [u8; 64] = [0; 64];
        data[0] = 0xff;
        data[9] = 0x7f;
        data[10] = greet.version.0;
        data[11] = greet.version.1;
        data[32] = greet.asServer.into();
        let mut bytes = BytesMut::new();
        bytes.extend_from_slice(&data);
        bytes
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Greeting(ZmqGreeting),
    Command(ZmqCommand),
    Bytes(Bytes)
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Message::Greeting(payload) => write!(f, "Greeting"),
            Message::Bytes(data) => write!(f, "Bin data - {}B", data.len()),
            Message::Command(c) => write!(f, "Command - {:?}", c)
        }
    }
}

enum DecoderState {
    Greeting,
    Handshake,
    Traffic
}

struct ZmqCodec {
    pub state: DecoderState
}

impl ZmqCodec {
    pub fn new() -> Self {
        Self { state: DecoderState::Greeting }
    }
}

impl Decoder for ZmqCodec {
    type Item = Message;
    type Error = ZmqError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        dbg!(&src);
        match self.state {
            DecoderState::Greeting => {
                if src.len() < 64 {
                    return if src[0] == 0xff {
                        Ok(None)
                    } else {
                        Err(ZmqError::CODEC("Bad first byte of greeting"))
                    }
                }
                self.state = DecoderState::Handshake;
                return Ok(Some(Greeting(ZmqGreeting::try_from(src.split_to(64).freeze())?)))
            },
            DecoderState::Handshake => {},
            DecoderState::Traffic => {},
        };
        Ok(None)
    }
}

impl Encoder for ZmqCodec {
    type Item = Message;
    type Error = ZmqError;

    fn encode(&mut self, message: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match message {
            Message::Greeting(payload) => dst.unsplit(payload.into()),
            Message::Bytes(data) => dst.extend_from_slice(&data),
            _ => {}
        }
        Ok(())
    }
}

pub struct Socket {
    _inner: Framed<TcpStream, ZmqCodec>,
}

impl Socket {
    pub async fn connect(endpoint: &str) -> ZmqResult<Self> {
        let addr = endpoint.parse::<SocketAddr>()?;
        let mut socket = Socket {
            _inner: Framed::new(TcpStream::connect(addr).await?, ZmqCodec::new()),
        };
        let mut data: [u8; 64] = [0; 64];
        data[0] = 0xff;
        data[9] = 0x7f;
        data[10] = 0x03;
        data[12..16].copy_from_slice("NULL".as_bytes());
        let greeting = Message::Bytes(Bytes::copy_from_slice(&data));
        socket.send(greeting, 0).await?;
        let message = socket.recv(0).await?;
        match &message {
            Greeting(payload) => {
                socket.send(message, 0).await?
            },
            _ => return Err(ZmqError::OTHER("Failed handshake".into()))
        };
        let message = socket.recv(0).await?;
        println!("{}", message);
        Ok(socket)
    }

    pub async fn send(&mut self, data: Message, flags: i32) -> ZmqResult<()> {
        self._inner.send(data).await
    }

    pub async fn recv(&mut self, flags: i32) -> ZmqResult<Message> {
        let mut buffer = [0 as u8; 1024];
        let message = self._inner.next().await;
        match message {
            Some(m) => Ok(m?),
            None => Err(ZmqError::NO_MESSAGE)
        }
    }
}

#[cfg(test)]
mod tests;
