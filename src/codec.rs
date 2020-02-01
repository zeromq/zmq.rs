use bytes::{BytesMut, Bytes};
use std::convert::TryFrom;
use crate::error::ZmqError;
use std::fmt::Display;

#[derive(Debug, Copy, Clone)]
pub enum ZmqMechanism {
    NULL,
    PLAIN,
    CURVE
}

impl Display for ZmqMechanism {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return match self {
            ZmqMechanism::NULL => write!(f, "NULL"),
            ZmqMechanism::PLAIN => write!(f, "PLAIN"),
            ZmqMechanism::CURVE => write!(f, "CURVE"),
        };
    }
}

impl TryFrom<Vec<u8>> for ZmqMechanism {
    type Error = ZmqError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        let mech = value.split(|x| *x == 0x0 ).next().unwrap_or(b"");
        // mechanism-char = "A"-"Z" | DIGIT
        //                  | "-" | "_" | "." | "+" | %x0
        // according to https://rfc.zeromq.org/spec:23/ZMTP/
        let mechanism = unsafe { String::from_utf8_unchecked(mech.to_vec()) };
        match mechanism.as_str() {
            "NULL" => Ok(ZmqMechanism::NULL),
            "PLAIN" => Ok(ZmqMechanism::PLAIN),
            "CURVE" => Ok(ZmqMechanism::CURVE),
            _ => Err(ZmqError::OTHER("Failed to parse ZmqMechanism"))
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ZmqGreeting {
    pub version: (u8, u8),
    pub mechanism: ZmqMechanism,
    pub asServer: bool
}

impl Default for ZmqGreeting {
    fn default() -> Self {
        Self {
            version: (3, 0),
            mechanism: ZmqMechanism::NULL,
            asServer: false
        }
    }
}

impl TryFrom<Bytes> for ZmqGreeting {
    type Error = ZmqError;

    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        if !(value[0] == 0xff && value[9] == 0x7f) {
            return Err(ZmqError::CODEC("Failed to parse greeting"))
        }
        Ok(ZmqGreeting {
            version: (value[10], value[11]),
            mechanism: ZmqMechanism::try_from(value[12..32].to_vec())?,
            asServer: value[32] == 0x01
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
        data[12..12+mech.len()].copy_from_slice(mech.as_bytes());
        data[32] = greet.asServer.into();
        let mut bytes = BytesMut::new();
        bytes.extend_from_slice(&data);
        bytes
    }
}