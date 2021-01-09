use crate::codec::{CodecError, Message, ZmtpVersion};
use crate::endpoint::Endpoint;
use crate::endpoint::EndpointError;
use crate::task_handle::TaskError;

use std::string::FromUtf8Error;
use thiserror::Error;

pub type ZmqResult<T> = Result<T, ZmqError>;

#[derive(Error, Debug)]
pub enum ZmqError {
    #[error("Endpoint Error: {0}")]
    Endpoint(#[from] EndpointError),
    #[error("Network Error: {0}")]
    Network(#[from] std::io::Error),
    #[error("Socket bind doesn't exist: {0}")]
    NoSuchBind(Endpoint),
    #[error("Codec Error: {0}")]
    Codec(#[from] CodecError),
    #[error("Socket Error: {0}")]
    Socket(&'static str),
    #[error("{0}")]
    BufferFull(&'static str),
    #[error("Failed to deliver message cause of {reason}")]
    ReturnToSender {
        reason: &'static str,
        message: Message,
    },
    #[error("Task Error: {0}")]
    Task(#[from] TaskError),
    #[error("{0}")]
    Other(&'static str),
    #[error("No message received")]
    NoMessage,
    #[error("Unsupported ZMTP version")]
    UnsupportedVersion(ZmtpVersion),
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

impl From<FromUtf8Error> for ZmqError {
    fn from(_e: FromUtf8Error) -> Self {
        // TODO wrap original error message
        ZmqError::Other("Failed to convert data to utf8 string")
    }
}
