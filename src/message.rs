use bytes::{Bytes, BytesMut};
use std::convert::TryFrom;
use std::string::FromUtf8Error;

/// A ZMQ message.
#[derive(Debug, Clone)]
pub struct ZmqMessage {
    /// The "raw" inner contents of the message.
    pub data: Bytes,
}

impl From<Bytes> for ZmqMessage {
    fn from(data: Bytes) -> Self {
        Self { data }
    }
}

impl From<BytesMut> for ZmqMessage {
    fn from(data: BytesMut) -> Self {
        data.freeze().into()
    }
}

impl From<Vec<u8>> for ZmqMessage {
    fn from(data: Vec<u8>) -> Self {
        Bytes::from(data).into()
    }
}

impl From<String> for ZmqMessage {
    fn from(data: String) -> Self {
        data.into_bytes().into()
    }
}

impl From<&str> for ZmqMessage {
    fn from(data: &str) -> Self {
        BytesMut::from(data).into()
    }
}

impl From<ZmqMessage> for Vec<u8> {
    fn from(m: ZmqMessage) -> Self {
        m.data.to_vec()
    }
}

impl TryFrom<ZmqMessage> for String {
    type Error = FromUtf8Error;

    fn try_from(m: ZmqMessage) -> Result<Self, Self::Error> {
        String::from_utf8(m.into())
    }
}
