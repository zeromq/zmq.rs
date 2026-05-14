use bytes::{BufMut, Bytes, BytesMut};

pub(crate) const GREETING_SIZE: usize = 64;

const MORE_FLAG: u8 = 0b0000_0001;
const LONG_FLAG: u8 = 0b0000_0010;
const COMMAND_FLAG: u8 = 0b0000_0100;
const SHORT_FRAME_LIMIT: usize = u8::MAX as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ZmtpFrameHeader {
    pub(crate) command: bool,
    pub(crate) long: bool,
    pub(crate) more: bool,
}

impl ZmtpFrameHeader {
    pub(crate) fn from_flags(flags: u8) -> Self {
        Self {
            command: (flags & COMMAND_FLAG) != 0,
            long: (flags & LONG_FLAG) != 0,
            more: (flags & MORE_FLAG) != 0,
        }
    }

    pub(crate) fn for_payload(payload_len: usize, command: bool, more: bool) -> Self {
        Self {
            command,
            long: payload_len > SHORT_FRAME_LIMIT,
            more,
        }
    }

    pub(crate) const fn length_size(self) -> usize {
        if self.long {
            8
        } else {
            1
        }
    }

    pub(crate) fn encoded_len(self, payload_len: usize) -> usize {
        1 + self.length_size() + payload_len
    }

    pub(crate) fn write_prefix(self, payload_len: usize, dst: &mut BytesMut) {
        debug_assert_eq!(self.long, payload_len > SHORT_FRAME_LIMIT);

        dst.reserve(self.encoded_len(payload_len));
        dst.put_u8(self.flag_byte());
        if self.long {
            dst.put_u64(payload_len as u64);
        } else {
            dst.put_u8(payload_len as u8);
        }
    }

    const fn flag_byte(self) -> u8 {
        let mut flags = 0;
        if self.more {
            flags |= MORE_FLAG;
        }
        if self.long {
            flags |= LONG_FLAG;
        }
        if self.command {
            flags |= COMMAND_FLAG;
        }
        flags
    }
}

pub(crate) fn encode_payload_frame(payload: &Bytes, dst: &mut BytesMut, command: bool, more: bool) {
    let header = ZmtpFrameHeader::for_payload(payload.len(), command, more);
    header.write_prefix(payload.len(), dst);
    dst.extend_from_slice(payload.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frame_flag_fixture() {
        let header = ZmtpFrameHeader::from_flags(COMMAND_FLAG | LONG_FLAG | MORE_FLAG);

        assert!(header.command);
        assert!(header.long);
        assert!(header.more);
        assert_eq!(header.length_size(), 8);
    }

    #[test]
    fn encodes_short_data_frame_fixture() {
        let payload = Bytes::from_static(b"abc");
        let mut out = BytesMut::new();

        encode_payload_frame(&payload, &mut out, false, true);

        assert_eq!(&out[..], &[MORE_FLAG, 3, b'a', b'b', b'c']);
    }

    #[test]
    fn encodes_short_command_frame_fixture() {
        let mut out = BytesMut::new();
        let header = ZmtpFrameHeader::for_payload(5, true, false);

        header.write_prefix(5, &mut out);
        out.extend_from_slice(b"READY");

        assert_eq!(&out[..], &[COMMAND_FLAG, 5, b'R', b'E', b'A', b'D', b'Y']);
    }

    #[test]
    fn encodes_long_data_frame_fixture() {
        let payload = Bytes::from(vec![0xab; 256]);
        let mut out = BytesMut::new();

        encode_payload_frame(&payload, &mut out, false, false);

        assert_eq!(out[0], LONG_FLAG);
        assert_eq!(&out[1..9], 256u64.to_be_bytes());
        assert_eq!(out.len(), 1 + 8 + payload.len());
        assert!(out[9..].iter().all(|byte| *byte == 0xab));
    }
}
