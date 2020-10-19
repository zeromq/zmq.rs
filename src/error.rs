use crate::codec::{CodecError, Message};
use crate::endpoint::EndpointError;
use crate::task_handle::TaskError;
use crate::ZmqMessage;

use thiserror::Error;

pub type ZmqResult<T> = Result<T, ZmqError>;

#[derive(Error, Debug)]
pub enum ZmqError {
    #[error("Endpoint Error: {0}")]
    Endpoint(#[from] EndpointError),
    #[error("Network Error: {0}")]
    Network(#[from] std::io::Error),
    #[error("Codec Error: {0}")]
    Codec(#[from] CodecError),
    #[error("Socket Error: {0}")]
    Socket(&'static str),
    #[error("{0}")]
    BufferFull(&'static str),
    #[error("Failed to deliver message cause of {reason}")]
    ReturnToSender {
        reason: &'static str,
        message: ZmqMessage,
    },
    #[error("Task Error: {0}")]
    Task(#[from] TaskError),
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
