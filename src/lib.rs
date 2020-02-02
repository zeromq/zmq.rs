#[macro_use]
extern crate enum_primitive_derive;
use num_traits::{FromPrimitive, ToPrimitive};

use bytes::{Buf, Bytes, BytesMut};
use futures_util::sink::SinkExt;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::prelude::*;
use tokio::stream::StreamExt;
use tokio_util::codec::Framed;
use tokio_util::codec::{Decoder, Encoder};

use crate::Message::{Command, Greeting};
use std::convert::TryFrom;
use std::fmt::Display;

mod codec;
mod error;

use crate::codec::*;
use crate::error::ZmqError;
use std::collections::HashMap;

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
enum ZmqCommandName {
    READY,
}

#[derive(Debug, Copy, Clone)]
pub struct ZmqCommand {
    name: ZmqCommandName,
}

impl TryFrom<BytesMut> for ZmqCommand {
    type Error = ZmqError;

    fn try_from(mut buf: BytesMut) -> Result<Self, Self::Error> {
        dbg!(&buf);
        let command_len = buf[0] as usize;
        buf.advance(1);
        // command-name-char = ALPHA according to https://rfc.zeromq.org/spec:23/ZMTP/
        let command_name =
            unsafe { String::from_utf8_unchecked(buf.split_to(command_len).to_vec()) };
        let command = match command_name.as_str() {
            "READY" => ZmqCommandName::READY,
            _ => return Err(ZmqError::CODEC("Uknown command received")),
        };

        let prop_len = buf[0] as usize;
        buf.advance(1);
        let property = unsafe { String::from_utf8_unchecked(buf.split_to(prop_len).to_vec()) };

        dbg!(property);
        Ok(Self { name: command })
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Greeting(ZmqGreeting),
    Command(ZmqCommand),
    Bytes(Bytes),
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Message::Greeting(payload) => write!(f, "Greeting"),
            Message::Bytes(data) => write!(f, "Bin data - {}B", data.len()),
            Message::Command(c) => write!(f, "Command - {:?}", c),
        }
    }
}

#[derive(Debug)]
enum DecoderState {
    Greeting,
    FrameHeader,
    Frame(bool, usize, bool),
    End,
}

struct ZmqCodec {
    pub state: DecoderState,
}

impl ZmqCodec {
    pub fn new() -> Self {
        Self {
            state: DecoderState::Greeting,
        }
    }
}

impl Decoder for ZmqCodec {
    type Item = Message;
    type Error = ZmqError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        dbg!(&self.state);
        dbg!(&src);
        return if src.is_empty() {
            Ok(None)
        } else {
            match self.state {
                DecoderState::Greeting => {
                    if src.len() >= 64 {
                        self.state = DecoderState::FrameHeader;
                        Ok(Some(Greeting(ZmqGreeting::try_from(
                            src.split_to(64).freeze(),
                        )?)))
                    } else {
                        if src[0] == 0xff {
                            src.reserve(64);
                            Ok(None)
                        } else {
                            Err(ZmqError::CODEC("Bad first byte of greeting"))
                        }
                    }
                }
                DecoderState::FrameHeader => {
                    let flags = src[0];
                    let command = (flags & 0b0000_0100) != 0;
                    let long = (flags & 0b0000_0010) != 0;
                    let more = ((flags & 0b0000_0001) != 0) | command;

                    let frame_len = if !long && src.len() >= 2 {
                        let command_len = src[1] as usize;
                        src.advance(2);
                        command_len
                    } else if long && src.len() >= 9 {
                        src.advance(1);
                        use std::usize;
                        usize::from_ne_bytes(unsafe {
                            *(src.split_to(8).as_ptr() as *const [u8; 8])
                        })
                    } else {
                        return Ok(None);
                    };
                    self.state = DecoderState::Frame(command, frame_len, more);
                    return self.decode(src);
                }
                DecoderState::Frame(command, frame_len, more) => {
                    if src.len() < frame_len {
                        src.reserve(frame_len);
                        return Ok(None);
                    }
                    if more {
                        self.state = DecoderState::FrameHeader;
                    } else {
                        self.state = DecoderState::End;
                    }
                    if command {
                        let message = Command(ZmqCommand::try_from(src.split_to(frame_len))?);
                        return Ok(Some(message));
                    }
                    // TODO process message frame
                    todo!()
                }
                DecoderState::End => Err(ZmqError::NO_MESSAGE),
            }
        };
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
        let greeting = Message::Greeting(ZmqGreeting::default());
        socket.send(greeting).await?;

        let greeting_repl = socket.recv().await?;
        dbg!(greeting_repl);

        let ready = b"\x04\x19\x05READY\x0bSocket-Type\0\0\0\x03REQ";
        socket
            .send(Message::Bytes(Bytes::from_static(ready)))
            .await?;

        let ready_repl = socket.recv().await?;
        dbg!(&ready_repl);

        Ok(socket)
    }

    pub async fn send(&mut self, data: Message) -> ZmqResult<()> {
        self._inner.send(data).await
    }

    pub async fn recv(&mut self) -> ZmqResult<Message> {
        let message = self._inner.next().await;
        match message {
            Some(m) => Ok(m?),
            None => Err(ZmqError::NO_MESSAGE),
        }
    }
}

#[cfg(test)]
mod tests;
