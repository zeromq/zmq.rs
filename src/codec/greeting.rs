use super::error::CodecError;
use super::mechanism::ZmqMechanism;

use bytes::{Bytes, BytesMut};
use std::convert::TryFrom;

#[derive(Debug, Copy, Clone)]
pub(crate) struct ZmqGreeting {
    pub version: (u8, u8),
    pub mechanism: ZmqMechanism,
    pub as_server: bool,
}

impl Default for ZmqGreeting {
    fn default() -> Self {
        Self {
            version: (3, 0),
            mechanism: ZmqMechanism::NULL,
            as_server: false,
        }
    }
}

impl TryFrom<Bytes> for ZmqGreeting {
    type Error = CodecError;

    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        if !(value[0] == 0xff && value[9] == 0x7f) {
            return Err(CodecError::Greeting("Failed to parse greeting"));
        }
        Ok(ZmqGreeting {
            version: (value[10], value[11]),
            mechanism: ZmqMechanism::try_from(value[12..32].to_vec())?,
            as_server: value[32] == 0x01,
        })
    }
}

impl From<ZmqGreeting> for BytesMut {
    fn from(greet: ZmqGreeting) -> Self {
        let mut data: [u8; 64] = [0; 64];
        data[0] = 0xff;
        data[9] = 0x7f;
        data[10] = greet.version.0;
        data[11] = greet.version.1;
        let mech = format!("{}", greet.mechanism);
        data[12..12 + mech.len()].copy_from_slice(mech.as_bytes());
        data[32] = greet.as_server.into();
        let mut bytes = BytesMut::new();
        bytes.extend_from_slice(&data);
        bytes
    }
}
