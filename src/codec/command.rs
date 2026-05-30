use super::error::CodecError;
use crate::SocketType;

use bytes::{Buf, BufMut, Bytes, BytesMut};

use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Display;

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Copy, Clone)]
pub enum ZmqCommandName {
    READY,
}

impl ZmqCommandName {
    pub const fn as_str(&self) -> &'static str {
        match self {
            ZmqCommandName::READY => "READY",
        }
    }
}

impl Display for ZmqCommandName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct ZmqCommand {
    pub name: ZmqCommandName,
    pub properties: HashMap<String, Bytes>,
}

impl ZmqCommand {
    pub fn ready(socket: SocketType) -> Self {
        let mut properties = HashMap::new();
        properties.insert("Socket-Type".into(), socket.as_str().into());
        Self {
            name: ZmqCommandName::READY,
            properties,
        }
    }

    pub fn add_prop(&mut self, name: String, value: Bytes) -> &mut Self {
        self.properties.insert(name, value);
        self
    }

    pub fn add_properties(&mut self, map: HashMap<String, Bytes>) -> &mut Self {
        self.properties.extend(map);
        self
    }
}

impl TryFrom<Bytes> for ZmqCommand {
    type Error = CodecError;

    fn try_from(mut buf: Bytes) -> Result<Self, Self::Error> {
        if buf.is_empty() {
            return Err(CodecError::Decode("Missing command length"));
        }

        let command_len = buf.get_u8() as usize;
        if buf.len() < command_len {
            return Err(CodecError::Decode("Command name exceeds frame length"));
        }

        // command-name-char = ALPHA according to https://rfc.zeromq.org/spec:23/ZMTP/
        let command = match &buf[..command_len] {
            b"READY" => ZmqCommandName::READY,
            _ => return Err(CodecError::Command("Unknown command received")),
        };
        buf.advance(command_len);
        let mut properties = HashMap::new();

        while !buf.is_empty() {
            // Collect command properties
            let prop_len = buf.get_u8() as usize;
            if buf.len() < prop_len {
                return Err(CodecError::Decode("Property name exceeds frame length"));
            }

            let property = match String::from_utf8(buf.split_to(prop_len).to_vec()) {
                Ok(p) => p,
                Err(_) => return Err(CodecError::Decode("Invalid property identifier")),
            };

            if buf.len() < 4 {
                return Err(CodecError::Decode("Missing property value length"));
            }

            let prop_val_len = buf.get_u32() as usize;
            if buf.len() < prop_val_len {
                return Err(CodecError::Decode("Property value exceeds frame length"));
            }

            let prop_value = buf.split_to(prop_val_len);
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
        let mut message_len = 0;

        let command_name = command.name.as_str();
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
            bytes.put_u64(message_len as u64);
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

#[cfg(test)]
mod tests {
    use super::ZmqCommand;
    use bytes::Bytes;
    use std::convert::TryFrom;

    fn assert_decode_error(data: &'static [u8]) {
        assert!(ZmqCommand::try_from(Bytes::from_static(data)).is_err());
    }

    #[test]
    fn rejects_truncated_command_name() {
        assert_decode_error(&[5, b'R']);
    }

    #[test]
    fn rejects_truncated_property_name() {
        assert_decode_error(&[5, b'R', b'E', b'A', b'D', b'Y', 11, b'S']);
    }

    #[test]
    fn rejects_truncated_property_value_length() {
        assert_decode_error(&[
            5, b'R', b'E', b'A', b'D', b'Y', 11, b'S', b'o', b'c', b'k', b'e', b't', b'-', b'T',
            b'y', b'p', b'e', 0, 0,
        ]);
    }

    #[test]
    fn rejects_truncated_property_value() {
        assert_decode_error(&[
            5, b'R', b'E', b'A', b'D', b'Y', 11, b'S', b'o', b'c', b'k', b'e', b't', b'-', b'T',
            b'y', b'p', b'e', 0, 0, 0, 3, b'R',
        ]);
    }
}
