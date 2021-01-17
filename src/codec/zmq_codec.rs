use super::command::ZmqCommand;
use super::error::CodecError;
use super::greeting::ZmqGreeting;
use super::Message;
use crate::ZmqMessage;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures_codec::{Decoder, Encoder};
use std::convert::TryFrom;

#[derive(Debug, Clone, Copy)]
struct Frame {
    command: bool,
    long: bool,
    more: bool,
}

#[derive(Debug)]
enum DecoderState {
    Greeting,
    FrameHeader,
    FrameLen(Frame),
    Frame(Frame),
}

#[derive(Debug)]
pub struct ZmqCodec {
    state: DecoderState,
    waiting_for: usize, // Number of bytes needed to decode frame
    // Needed to store incoming multipart message
    // This allows to incapsulate it's processing inside codec and not expose
    // internal details to higher levels
    buffered_message: Option<ZmqMessage>,
}

impl ZmqCodec {
    pub fn new() -> Self {
        Self {
            state: DecoderState::Greeting,
            waiting_for: 64, // len of the greeting frame,
            buffered_message: None,
        }
    }
}

impl Default for ZmqCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder for ZmqCodec {
    type Error = CodecError;
    type Item = Message;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < self.waiting_for {
            src.reserve(self.waiting_for - src.len());
            return Ok(None);
        }
        match self.state {
            DecoderState::Greeting => {
                if src[0] != 0xff {
                    return Err(CodecError::Decode("Bad first byte of greeting"));
                }
                self.state = DecoderState::FrameHeader;
                self.waiting_for = 1;
                Ok(Some(Message::Greeting(ZmqGreeting::try_from(
                    src.split_to(64).freeze(),
                )?)))
            }
            DecoderState::FrameHeader => {
                let flags = src.get_u8();

                let frame = Frame {
                    command: (flags & 0b0000_0100) != 0,
                    long: (flags & 0b0000_0010) != 0,
                    more: (flags & 0b0000_0001) != 0,
                };
                self.state = DecoderState::FrameLen(frame);
                self.waiting_for = if frame.long { 8 } else { 1 };
                self.decode(src)
            }
            DecoderState::FrameLen(frame) => {
                self.state = DecoderState::Frame(frame);
                self.waiting_for = if frame.long {
                    src.get_u64() as usize
                } else {
                    src.get_u8() as usize
                };
                self.decode(src)
            }
            DecoderState::Frame(frame) => {
                let data = src.split_to(self.waiting_for);
                self.state = DecoderState::FrameHeader;
                self.waiting_for = 1;
                if frame.command {
                    return Ok(Some(Message::Command(ZmqCommand::try_from(data)?)));
                }

                // process incoming message frame
                match &mut self.buffered_message {
                    Some(v) => v.push_back(data.freeze()),
                    None => self.buffered_message = Some(ZmqMessage::from(data.freeze())),
                }

                if frame.more {
                    self.decode(src)
                } else {
                    // Quoth the Raven “Nevermore.”
                    Ok(Some(Message::Message(
                        self.buffered_message
                            .take()
                            .expect("Corrupted decoder state"),
                    )))
                }
            }
        }
    }
}

impl ZmqCodec {
    fn _encode_frame(&mut self, frame: &Bytes, dst: &mut BytesMut, more: bool) {
        let mut flags: u8 = 0;
        if more {
            flags |= 0b0000_0001;
        }
        let len = frame.len();
        if len > 255 {
            flags |= 0b0000_0010;
            dst.reserve(len + 9);
        } else {
            dst.reserve(len + 2);
        }
        dst.put_u8(flags);
        if len > 255 {
            dst.put_u64(len as u64);
        } else {
            dst.put_u8(len as u8);
        }
        dst.extend_from_slice(frame.as_ref());
    }
}

impl Encoder for ZmqCodec {
    type Error = CodecError;
    type Item = Message;

    fn encode(&mut self, message: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match message {
            Message::Greeting(payload) => dst.unsplit(payload.into()),
            Message::Command(command) => dst.unsplit(command.into()),
            Message::Message(message) => {
                let last_element = message.len() - 1;
                for (idx, part) in message.iter().enumerate() {
                    self._encode_frame(part, dst, idx != last_element);
                }
            }
        }
        Ok(())
    }
}
