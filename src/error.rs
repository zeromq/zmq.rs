use std::error::Error;
use std::fmt::Display;

#[derive(Debug)]
pub enum ZmqError {
    NETWORK(String),
    CODEC(&'static str),
    SOCKET(&'static str),
    OTHER(&'static str),
    NO_MESSAGE,
}

impl Display for ZmqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return match self {
            ZmqError::NETWORK(message) => write!(f, "{}", message),
            ZmqError::OTHER(message) => write!(f, "{}", message),
            ZmqError::CODEC(message) => write!(f, "{}", message),
            ZmqError::SOCKET(message) => write!(f, "{}", message),
            ZmqError::NO_MESSAGE => write!(f, "No data received"),
        };
    }
}

impl Error for ZmqError {}

impl From<std::net::AddrParseError> for ZmqError {
    fn from(reason: std::net::AddrParseError) -> Self {
        ZmqError::NETWORK(reason.to_string())
    }
}
impl From<std::io::Error> for ZmqError {
    fn from(reason: std::io::Error) -> Self {
        ZmqError::NETWORK(reason.to_string())
    }
}
