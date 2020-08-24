use crate::codec::Message;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ZmqError {
    #[error("Malformed socket address")]
    Address(#[from] std::net::AddrParseError),
    #[error("Network error")]
    Network(#[from] std::io::Error),
    #[error("{0}")]
    Codec(&'static str),
    #[error("{0}")]
    Socket(&'static str),
    #[error("{0}")]
    BufferFull(&'static str),
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
