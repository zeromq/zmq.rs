mod command;
mod greeting;
mod mechanism;
mod zmq_codec;

pub(crate) use command::{ZmqCommand, ZmqCommandName};
pub(crate) use greeting::ZmqGreeting;
pub(crate) use zmq_codec::ZmqCodec;

use crate::message::*;

#[derive(Debug, Clone)]
pub(crate) enum Message {
    Greeting(ZmqGreeting),
    Command(ZmqCommand),
    Message(ZmqMessage),
    Multipart(Vec<ZmqMessage>),
}
