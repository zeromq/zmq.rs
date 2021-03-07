use super::error::CodecError;
use crate::SocketType;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::collections::HashMap;
use std::convert::TryFrom;

#[derive(Debug, Copy, Clone)]
pub enum ZmqCommandName {
    READY,
}

impl From<ZmqCommandName> for String {
    fn from(c_name: ZmqCommandName) -> Self {
        match c_name {
            ZmqCommandName::READY => "READY".into(),
        }
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
        properties.insert("Socket-Type".into(), format!("{}", socket).into());
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

impl TryFrom<BytesMut> for ZmqCommand {
    type Error = CodecError;

    fn try_from(mut buf: BytesMut) -> Result<Self, Self::Error> {
        let command_len = buf.get_u8() as usize;
        // command-name-char = ALPHA according to https://rfc.zeromq.org/spec:23/ZMTP/
        let command_name =
            unsafe { String::from_utf8_unchecked(buf.split_to(command_len).to_vec()) };
        let command = match command_name.as_str() {
            "READY" => ZmqCommandName::READY,
            _ => return Err(CodecError::Command("Uknown command received")),
        };
        let mut properties = HashMap::new();

        while !buf.is_empty() {
            // Collect command properties
            let prop_len = buf.get_u8() as usize;
            let property = unsafe { String::from_utf8_unchecked(buf.split_to(prop_len).to_vec()) };

            let prop_val_len = buf.get_u32() as usize;
            let prop_value = buf.split_to(prop_val_len).freeze();
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
