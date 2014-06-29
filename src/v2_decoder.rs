use consts;
use msg;
use msg::Msg;
use result::{ZmqError, ZmqResult};
use v2_protocol::{MORE_FLAG, LARGE_FLAG, COMMAND_FLAG};

use std::io::Reader;


pub struct V2Decoder {
    maxmsgsize: i64,
}

impl V2Decoder {
    pub fn new(maxmsgsize: i64) -> V2Decoder {
        V2Decoder {
            maxmsgsize: maxmsgsize,
        }
    }

    pub fn decode(&self, reader: &mut Reader) -> ZmqResult<Box<Msg>> {
        let mut msg_flags = 0u8;
        let flags = try!(reader.read_byte().map_err(ZmqError::from_io_error));
        if flags & MORE_FLAG != 0 {
            msg_flags |= msg::MORE;
        }
        if flags & COMMAND_FLAG != 0 {
            msg_flags |= msg::COMMAND;
        }

        let msg_size =
            if flags & LARGE_FLAG != 0 {
                let size = try!(reader.read_be_u64().map_err(ZmqError::from_io_error));

                //  Message size must not exceed the maximum allowed size.
                if self.maxmsgsize > 0 && size > self.maxmsgsize as u64 {
                    return Err(ZmqError::new(consts::EMSGSIZE, "Message too long"));
                }

                //  Message size must fit into uint data type.
                if size != (size as uint) as u64 {
                    return Err(ZmqError::new(consts::EMSGSIZE, "Message too long"));
                }

                size as uint
            } else {
                let size = try!(reader.read_byte().map_err(ZmqError::from_io_error));

                //  Message size must not exceed the maximum allowed size.
                if self.maxmsgsize > 0 && size as u64 > self.maxmsgsize as u64 {
                    return Err(ZmqError::new(consts::EMSGSIZE, "Message too long"));
                }

                size as uint
            };

        let mut ret = box Msg::new(msg_size);
        ret.flags = msg_flags;

        if msg_size > 0 {
            let n = try!(reader.push_at_least(
                msg_size, msg_size, &mut ret.data).map_err(ZmqError::from_io_error));
            assert_eq!(msg_size, n);
        }

        Ok(ret)
    }
}
