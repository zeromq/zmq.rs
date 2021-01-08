use bytes::Bytes;
use std::collections::vec_deque::{VecDeque,Iter};
use std::convert::{From, TryFrom};

#[derive(Debug, Clone)]
pub struct ZmqMessage {
    frames: VecDeque<Bytes>,
}

impl ZmqMessage {
    pub fn new() -> Self {
	Self { frames: VecDeque::new() }
    }

    pub fn push_back(&mut self, frame: Bytes) {
	self.frames.push_back(frame);
    }
    
    pub fn push_front(&mut self, frame: Bytes) {
	self.frames.push_front(frame);
    }

    pub fn iter(&self) -> Iter<'_, Bytes> {
	self.frames.iter()
    }

    pub fn pop_front(&mut self) -> Option<Bytes> {
	self.frames.pop_front()
    }

    pub fn pop_back(&mut self) -> Option<Bytes> {
	self.frames.pop_back()
    }

    pub fn len(&self) -> usize {
	self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
	self.frames.is_empty()
    }
    
    pub fn get(&self, index: usize) -> Option<&Bytes> {
	self.frames.get(index)
    }
}

impl From<Vec<Bytes>> for ZmqMessage {
    fn from(v: Vec<Bytes>) -> Self {
	Self { frames: v.into() }
    }
}

impl From<String> for ZmqMessage {
    fn from(s: String) -> Self {
	let mut m: ZmqMessage = Default::default();
	m.push_back(s.into());
	m
    }
}

impl From<&str> for ZmqMessage {
    fn from(s: &str) -> Self {
	ZmqMessage::from(s.to_owned())
    } 
}

impl Default for ZmqMessage {
    fn default() -> Self {
	Self::new()
    }
}

impl TryFrom<ZmqMessage> for String {
    type Error = &'static str;
    
    fn try_from(mut z: ZmqMessage) -> Result<Self, Self::Error> {
	if z.len() != 1 {
	    return Err("Message must have only 1 frame to convert to String");
	}
	match String::from_utf8(z.pop_front().unwrap().to_vec()) {
	    Ok(s) => Ok(s),
	    Err(_) => Err("Could not parse string from message"),
	}
    }
}
