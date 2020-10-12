use super::command::ZmqCommand;
use super::greeting::ZmqGreeting;
use super::Message;
use crate::error::ZmqError;
use crate::ZmqMessage;

use bytes::{Buf, BufMut, BytesMut};
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
pub(crate) struct ZmqCodec {
    state: DecoderState,
    waiting_for: usize, // Number of bytes needed to decode frame
    // Needed to store incoming multipart message
    // This allows to incapsulate it's processing inside codec and not expose
    // internal details to higher levels
    buffered_message: Option<Message>,
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

impl Decoder for ZmqCodec {
    type Error = ZmqError;
    type Item = Message;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < self.waiting_for {
            src.reserve(self.waiting_for - src.len());
            return Ok(None);
        }
        match self.state {
            DecoderState::Greeting => {
                if src[0] != 0xff {
                    return Err(ZmqError::Codec("Bad first byte of greeting"));
                }
                self.state = DecoderState::FrameHeader;
                self.waiting_for = 1;
                Ok(Some(Message::Greeting(ZmqGreeting::try_from(
                    src.split_to(64).freeze(),
                )?)))
            }
            DecoderState::FrameHeader => {
                let flags = src.get_u8();
                let command = (flags & 0b0000_0100) != 0;
                let long = (flags & 0b0000_0010) != 0;
                let more = (flags & 0b0000_0001) != 0;

                if more && self.buffered_message.is_none() {
                    self.buffered_message = Some(Message::Multipart(Vec::new()));
                }
                let frame = Frame {
                    command,
                    long,
                    more,
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
                    Ok(Some(Message::Command(ZmqCommand::try_from(data)?)))
                } else if frame.more {
                    // cache incoming multipart message
                    match &mut self.buffered_message {
                        Some(Message::Multipart(message)) => message.push(data.into()),
                        _ => panic!("Corrupted decoder state"),
                    }
                    self.decode(src)
                } else if let Some(Message::Multipart(mut message)) = self.buffered_message.take() {
                    message.push(data.into());
                    Ok(Some(Message::Multipart(message)))
                } else {
                    Ok(Some(Message::Message(data.into())))
                }
            }
        }
    }
}

impl ZmqCodec {
    fn _encode_message(&mut self, message: ZmqMessage, dst: &mut BytesMut, more: bool) {
        let mut flags: u8 = 0;
        if more {
            flags |= 0b0000_0001;
        }
        let len = message.data.len();
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
        dst.extend_from_slice(message.data.as_ref());
    }
}

impl Encoder for ZmqCodec {
    type Error = ZmqError;
    type Item = Message;

    fn encode(&mut self, message: Self::Item, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match message {
            Message::Greeting(payload) => dst.unsplit(payload.into()),
            Message::Message(message) => self._encode_message(message, dst, false),
            Message::Command(command) => dst.unsplit(command.into()),
            Message::Multipart(parts) => {
                let last_element = parts.len() - 1;
                for (idx, part) in parts.into_iter().enumerate() {
                    self._encode_message(part, dst, idx != last_element);
                }
            }
        }
        Ok(())
    }
}
