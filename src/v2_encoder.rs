use msg;
use msg::Msg;
use v2_protocol::{MORE_FLAG, LARGE_FLAG, COMMAND_FLAG};

use std::io::{IoResult, Writer};


pub struct V2Encoder;

impl V2Encoder {
    pub fn new() -> V2Encoder {
        V2Encoder
    }

    pub fn encode(&self, msg: Box<Msg>, writer: &mut Writer) -> IoResult<()> {
        // send protocol flags
        let mut protocol_flags = 0u8;
        if msg.flags & msg::MORE != 0 {
            protocol_flags |= MORE_FLAG;
        }
        if msg.data.len() > 255 {
            protocol_flags |= LARGE_FLAG;
        }
        if msg.flags & msg::COMMAND != 0 {
            protocol_flags |= COMMAND_FLAG;
        }
        try!(writer.write_u8(protocol_flags));

        // send size
        if msg.data.len() > 255 {
            try!(writer.write_be_u64(msg.data.len() as u64));
        } else {
            try!(writer.write_u8(msg.data.len() as u8));
        }

        // send msg body
        try!(writer.write(msg.data.as_slice()));

        Ok(())
    }
}