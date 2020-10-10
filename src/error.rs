use crate::codec::{CodecError, Message};
use crate::ZmqMessage;

use thiserror::Error;

pub type ZmqResult<T> = Result<T, ZmqError>;

#[derive(Error, Debug)]
pub enum ZmqError {
    #[error(transparent)]
    Endpoint(#[from] EndpointError),
    #[error("Network error")]
    Network(#[from] std::io::Error),
    #[error("Codec Error: {0}")]
    Codec(#[from] CodecError),
    #[error("{0}")]
    Socket(&'static str),
    #[error("{0}")]
    BufferFull(&'static str),
    #[error("Failed to deliver message cause of {reason}")]
    ReturnToSender {
        reason: &'static str,
        message: ZmqMessage,
    },
    #[error("{0}")]
    Other(&'static str),
    #[error("No message received")]
    NoMessage,
}

impl From<futures::channel::mpsc::TrySendError<Message>> for ZmqError {
    fn from(_: futures::channel::mpsc::TrySendError<Message>) -> Self {
        ZmqError::BufferFull("Failed to send message. Send queue full/broken")
    }
}

impl From<futures::channel::mpsc::SendError> for ZmqError {
    fn from(_: futures::channel::mpsc::SendError) -> Self {
        ZmqError::BufferFull("Failed to send message. Send queue full/broken")
    }
}

/// Represents an error when parsing an [`Endpoint`]
#[derive(Error, Debug)]
pub enum EndpointError {
    #[error("Failed to parse IP address or port")]
    ParseIpAddr(#[from] std::net::AddrParseError),
    #[error("Unknown transport type {0}")]
    UnknownTransport(String),
    #[error("Invalid Syntax: {0}")]
    Syntax(&'static str),
}
