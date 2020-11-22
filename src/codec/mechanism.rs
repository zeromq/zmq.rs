use super::error::CodecError;

use std::convert::TryFrom;
use std::fmt::Display;

#[derive(Debug, Copy, Clone)]
pub enum ZmqMechanism {
    NULL,
    PLAIN,
    CURVE,
}

impl Display for ZmqMechanism {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ZmqMechanism::NULL => write!(f, "NULL"),
            ZmqMechanism::PLAIN => write!(f, "PLAIN"),
            ZmqMechanism::CURVE => write!(f, "CURVE"),
        }
    }
}

impl TryFrom<Vec<u8>> for ZmqMechanism {
    type Error = CodecError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        let mech = value.split(|x| *x == 0x0).next().unwrap_or(b"");
        // mechanism-char = "A"-"Z" | DIGIT
        //                  | "-" | "_" | "." | "+" | %x0
        // according to https://rfc.zeromq.org/spec:23/ZMTP/
        let mechanism = unsafe { String::from_utf8_unchecked(mech.to_vec()) };
        match mechanism.as_str() {
            "NULL" => Ok(ZmqMechanism::NULL),
            "PLAIN" => Ok(ZmqMechanism::PLAIN),
            "CURVE" => Ok(ZmqMechanism::CURVE),
            _ => Err(CodecError::Mechanism("Failed to parse ZmqMechanism")),
        }
    }
}
