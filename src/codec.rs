use crate::error::ZmqError;
use bytes::{Bytes, BytesMut, Buf, BufMut};
use std::convert::TryFrom;
use std::fmt::Display;
use std::collections::HashMap;
use crate::SocketType;

#[derive(Debug, Copy, Clone)]
pub enum ZmqMechanism {
    NULL,
    PLAIN,
    CURVE,
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
        let mech = value.split(|x| *x == 0x0).next().unwrap_or(b"");
        // mechanism-char = "A"-"Z" | DIGIT
        //                  | "-" | "_" | "." | "+" | %x0
        // according to https://rfc.zeromq.org/spec:23/ZMTP/
        let mechanism = unsafe { String::from_utf8_unchecked(mech.to_vec()) };
        match mechanism.as_str() {
            "NULL" => Ok(ZmqMechanism::NULL),
            "PLAIN" => Ok(ZmqMechanism::PLAIN),
            "CURVE" => Ok(ZmqMechanism::CURVE),
            _ => Err(ZmqError::OTHER("Failed to parse ZmqMechanism")),
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub struct ZmqGreeting {
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
    type Error = ZmqError;

    fn try_from(value: Bytes) -> Result<Self, Self::Error> {
        if !(value[0] == 0xff && value[9] == 0x7f) {
            return Err(ZmqError::CODEC("Failed to parse greeting"));
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


#[derive(Debug, Copy, Clone)]
pub enum ZmqCommandName {
    READY,
}

impl From<ZmqCommandName> for String {
    fn from(c_name: ZmqCommandName) -> Self {
        match c_name {
            ZmqCommandName::READY => { "READY".into() },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ZmqCommand {
    pub name: ZmqCommandName,
    pub properties: HashMap<String, String>,
}

impl ZmqCommand {
    pub fn ready(socket: SocketType) -> Self {
        let mut properties = HashMap::new();
        properties.insert("Socket-Type".into(), format!("{}", socket));
        Self {
            name: ZmqCommandName::READY,
            properties,
        }
    }
}

impl TryFrom<BytesMut> for ZmqCommand {
    type Error = ZmqError;

    fn try_from(mut buf: BytesMut) -> Result<Self, Self::Error> {
        let command_len = buf[0] as usize;
        buf.advance(1);
        // command-name-char = ALPHA according to https://rfc.zeromq.org/spec:23/ZMTP/
        let command_name =
            unsafe { String::from_utf8_unchecked(buf.split_to(command_len).to_vec()) };
        let command = match command_name.as_str() {
            "READY" => ZmqCommandName::READY,
            _ => return Err(ZmqError::CODEC("Uknown command received")),
        };
        let mut properties = HashMap::new();

        while !buf.is_empty() {
            // Collect command properties
            let prop_len = buf[0] as usize;
            buf.advance(1);
            let property = unsafe { String::from_utf8_unchecked(buf.split_to(prop_len).to_vec()) };

            use std::u32;
            let prop_val_len = unsafe {
                u32::from_be_bytes(*(buf.split_to(4).as_ptr() as *const [u8; 4])) as usize
            };
            let prop_value =
                unsafe { String::from_utf8_unchecked(buf.split_to(prop_val_len).to_vec()) };
            properties.insert(property, prop_value);
        }
        Ok(Self {
            name: command,
            properties,
        })
    }
}

impl From<ZmqCommand> for BytesMut {
    fn from(command: ZmqCommand) -> Self {
        use std::usize;
        let mut message_len = 0;

        let command_name: String = command.name.into();
        message_len += command_name.len() + 1;
        for (prop, val) in command.properties.iter() {
            message_len += prop.len() + 1;
            message_len += val.len() + 4;
        }

        let long_message = message_len > 255;

        let mut bytes = BytesMut::new();
        if long_message {
            bytes.reserve(message_len + 9);
            bytes.put_u8(0x06);
            bytes.extend_from_slice( &message_len.to_be_bytes());
        } else {
            bytes.reserve(message_len + 2);
            bytes.put_u8(0x04);
            bytes.put_u8(message_len as u8);
        };
        bytes.put_u8(command_name.len() as u8);
        bytes.extend_from_slice(command_name.as_ref());
        for (prop, val) in command.properties.iter() {
            bytes.put_u8(prop.len() as u8);
            bytes.extend_from_slice(prop.as_ref());
            bytes.put_u32(val.len() as u32);
            bytes.extend_from_slice(val.as_ref());
        }
        bytes
    }
}
