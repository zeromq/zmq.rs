//! Implements a codec for ZMQ, providing a way to convert from a byte-oriented
//! io device to a protocal comprised of [`Message`] frames. See [`FramedIo`]

mod command;
mod error;
mod framed;
mod greeting;
pub(crate) mod mechanism;
mod zmq_codec;

pub(crate) use command::{ZmqCommand, ZmqCommandName};
pub(crate) use error::{CodecError, CodecResult};
pub(crate) use framed::{FrameableRead, FrameableWrite, FramedIo, ZmqFramedRead, ZmqFramedWrite};
pub(crate) use greeting::{ZmqGreeting, ZmtpVersion};
pub(crate) use zmq_codec::ZmqCodec;

use crate::message::ZmqMessage;
use crate::util::PeerIdentity;
use crate::{ZmqError, ZmqResult};
use bytes::{Bytes, BytesMut};
use futures::task::Poll;
use futures::Sink;
use std::convert::TryFrom;
use std::pin::Pin;

#[derive(Debug, Clone)]
pub enum Message {
    Greeting(ZmqGreeting),
    Command(ZmqCommand),
    Message(ZmqMessage),
    Multipart(Vec<ZmqMessage>),
}

impl From<Bytes> for Message {
    fn from(data: Bytes) -> Self {
        Message::Message(data.into())
    }
}

impl From<BytesMut> for Message {
    fn from(data: BytesMut) -> Self {
        data.freeze().into()
    }
}

impl From<Vec<u8>> for Message {
    fn from(data: Vec<u8>) -> Self {
        Bytes::from(data).into()
    }
}

impl From<String> for Message {
    fn from(data: String) -> Self {
        data.into_bytes().into()
    }
}

impl From<Vec<Vec<u8>>> for Message {
    fn from(data: Vec<Vec<u8>>) -> Self {
        Message::Multipart(data.into_iter().map(ZmqMessage::from).collect())
    }
}

impl From<Vec<String>> for Message {
    fn from(data: Vec<String>) -> Self {
        Message::Multipart(data.into_iter().map(ZmqMessage::from).collect())
    }
}

impl From<PeerIdentity> for Message {
    fn from(data: PeerIdentity) -> Self {
        Bytes::from(data).into()
    }
}

impl From<&str> for Message {
    fn from(data: &str) -> Self {
        BytesMut::from(data).into()
    }
}

impl TryFrom<Message> for Vec<u8> {
    type Error = ZmqError;

    fn try_from(m: Message) -> Result<Self, Self::Error> {
        match m {
            Message::Message(m) => Ok(m.data.to_vec()),
            _ => Err(ZmqError::Other(
                "Unable to implicitly cast message to appropriate type",
            )),
        }
    }
}

impl TryFrom<Message> for String {
    type Error = ZmqError;

    fn try_from(m: Message) -> Result<Self, Self::Error> {
        match m {
            Message::Message(m) => Ok(String::from_utf8(m.into())?),
            _ => Err(ZmqError::Other(
                "Unable to implicitly cast message to appropriate type",
            )),
        }
    }
}

pub(crate) trait TrySend {
    fn try_send(self: Pin<&mut Self>, item: Message) -> ZmqResult<()>;
}

impl TrySend for ZmqFramedWrite {
    fn try_send(mut self: Pin<&mut Self>, item: Message) -> ZmqResult<()> {
        let waker = futures::task::noop_waker();
        let mut cx = futures::task::Context::from_waker(&waker);
        match self.as_mut().poll_ready(&mut cx) {
            Poll::Ready(Ok(())) => {
                self.as_mut().start_send(item)?;
                let _ = self.as_mut().poll_flush(&mut cx); // ignore result just hope that it flush eventually
                Ok(())
            }
            Poll::Ready(Err(e)) => Err(e.into()),
            Poll::Pending => Err(ZmqError::BufferFull("Sink is full")),
        }
    }
}
