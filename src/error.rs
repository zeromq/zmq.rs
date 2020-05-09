use std::error::Error;
use std::fmt::Display;

#[derive(Debug)]
pub enum ZmqError {
    Network(String),
    Codec(&'static str),
    Socket(&'static str),
    Other(&'static str),
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
