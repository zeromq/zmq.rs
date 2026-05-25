//! Implements a codec for ZMQ, providing a way to convert from a byte-oriented
//! io device to a protocal comprised of [`Message`] frames. See [`FramedIo`]

mod command;
mod error;
mod framed;
mod greeting;
pub(crate) mod mechanism;
mod zmq_codec;

pub(crate) use command::{ZmqCommand, ZmqCommandName};
pub use error::CodecError;
pub(crate) use error::CodecResult;
pub use framed::ZmqFramedRead;
pub(crate) use framed::{FrameableWrite, FramedIo, ZmqFramedWrite};
pub(crate) use greeting::{ZmqGreeting, ZmtpVersion};
pub use zmq_codec::ZmqCodec;

use crate::message::ZmqMessage;

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
pub enum Message {
    Greeting(ZmqGreeting),
    Command(ZmqCommand),
    Message(ZmqMessage),
}
