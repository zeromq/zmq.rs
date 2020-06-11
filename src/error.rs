use crate::codec::Message;
use std::error::Error;
use std::fmt::Display;

/// A library error.
#[derive(Debug)]
pub enum ZmqError {
    /// A network related error (typically IO)
    Network(String),

    /// A codec related error.
    Codec(&'static str),

    /// A socket related error.
    Socket(&'static str),

    /// Some other error.
    Other(&'static str),

    /// No data was received.
    NoMessage,
}

impl Display for ZmqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return match self {
            ZmqError::Network(message) => write!(f, "{}", message),
            ZmqError::Other(message) => write!(f, "{}", message),
            ZmqError::Codec(message) => write!(f, "{}", message),
            ZmqError::Socket(message) => write!(f, "{}", message),
            ZmqError::NoMessage => write!(f, "No data received"),
        };
    }
}

impl Error for ZmqError {}

impl From<std::net::AddrParseError> for ZmqError {
    fn from(reason: std::net::AddrParseError) -> Self {
        ZmqError::Network(reason.to_string())
    }
}
impl From<std::io::Error> for ZmqError {
    fn from(reason: std::io::Error) -> Self {
        ZmqError::Network(reason.to_string())
    }
}

impl From<futures::channel::mpsc::TrySendError<Message>> for ZmqError {
    fn from(_: futures::channel::mpsc::TrySendError<Message>) -> Self {
        ZmqError::Other("Failed to send message. Send queue full")
    }
}
