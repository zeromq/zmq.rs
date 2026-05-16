use bytes::Bytes;

use std::collections::VecDeque;
use std::convert::{From, TryFrom};
use std::fmt;
use std::iter::FusedIterator;
use std::sync::OnceLock;

#[derive(Debug)]
pub struct ZmqEmptyMessageError;

impl fmt::Display for ZmqEmptyMessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unable to construct an empty ZmqMessage")
    }
}

#[derive(Debug)]
pub struct ZmqMessage {
    frames: MessageFrames,
}

#[derive(Debug)]
enum MessageFrames {
    Empty,
    #[allow(clippy::box_collection)]
    Single {
        frame: Bytes,
        // Public API requires returning VecDeque::Iter. Keep that compatibility
        // cache off the hot message path and allocate it only when public iter()
        // is used on a single-frame message.
        iter_cache: OnceLock<Box<VecDeque<Bytes>>>,
    },
    Multi(VecDeque<Bytes>),
}

static EMPTY_FRAMES: OnceLock<VecDeque<Bytes>> = OnceLock::new();

#[derive(Debug, Clone)]
enum IterInner<'a> {
    Empty,
    Single(Option<&'a Bytes>),
    Multi(std::collections::vec_deque::Iter<'a, Bytes>),
}

#[derive(Debug, Clone)]
struct IterRef<'a> {
    inner: IterInner<'a>,
}

impl<'a> Iterator for IterRef<'a> {
    type Item = &'a Bytes;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            IterInner::Empty => None,
            IterInner::Single(frame) => frame.take(),
            IterInner::Multi(iter) => iter.next(),
        }
    }
}

impl DoubleEndedIterator for IterRef<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            IterInner::Empty => None,
            IterInner::Single(frame) => frame.take(),
            IterInner::Multi(iter) => iter.next_back(),
        }
    }
}

impl ExactSizeIterator for IterRef<'_> {
    fn len(&self) -> usize {
        match &self.inner {
            IterInner::Empty => 0,
            IterInner::Single(frame) => usize::from(frame.is_some()),
            IterInner::Multi(iter) => iter.len(),
        }
    }
}

impl FusedIterator for IterRef<'_> {}

impl MessageFrames {
    fn single(frame: Bytes) -> Self {
        Self::Single {
            frame,
            iter_cache: OnceLock::new(),
        }
    }

    fn from_vecdeque(mut frames: VecDeque<Bytes>) -> Self {
        match frames.len() {
            0 => Self::Empty,
            1 => Self::single(frames.pop_front().expect("length checked")),
            _ => Self::Multi(frames),
        }
    }

    fn from_vec(mut frames: Vec<Bytes>) -> Self {
        match frames.len() {
            0 => Self::Empty,
            1 => Self::single(frames.pop().expect("length checked")),
            _ => Self::Multi(frames.into()),
        }
    }

    fn normalize(&mut self) {
        let replacement = match self {
            Self::Multi(frames) if frames.is_empty() => Some(Self::Empty),
            Self::Multi(frames) if frames.len() == 1 => {
                Some(Self::single(frames.pop_front().expect("length checked")))
            }
            _ => None,
        };

        if let Some(replacement) = replacement {
            *self = replacement;
        }
    }

    fn public_iter(&self) -> std::collections::vec_deque::Iter<'_, Bytes> {
        match self {
            Self::Empty => EMPTY_FRAMES.get_or_init(VecDeque::new).iter(),
            Self::Single { frame, iter_cache } => iter_cache
                .get_or_init(|| {
                    let mut frames = VecDeque::with_capacity(1);
                    frames.push_back(frame.clone());
                    Box::new(frames)
                })
                .iter(),
            Self::Multi(frames) => frames.iter(),
        }
    }
}

impl Clone for ZmqMessage {
    fn clone(&self) -> Self {
        Self {
            frames: self.frames.clone(),
        }
    }
}

impl Clone for MessageFrames {
    fn clone(&self) -> Self {
        match self {
            Self::Empty => Self::Empty,
            Self::Single { frame, .. } => Self::single(frame.clone()),
            Self::Multi(frames) => Self::Multi(frames.clone()),
        }
    }
}

impl ZmqMessage {
    pub fn push_back(&mut self, frame: Bytes) {
        match std::mem::replace(&mut self.frames, MessageFrames::Empty) {
            MessageFrames::Empty => self.frames = MessageFrames::single(frame),
            MessageFrames::Single {
                frame: existing, ..
            } => {
                let mut frames = VecDeque::with_capacity(2);
                frames.push_back(existing);
                frames.push_back(frame);
                self.frames = MessageFrames::Multi(frames);
            }
            MessageFrames::Multi(mut frames) => {
                frames.push_back(frame);
                self.frames = MessageFrames::Multi(frames);
            }
        }
    }

    pub fn push_front(&mut self, frame: Bytes) {
        match std::mem::replace(&mut self.frames, MessageFrames::Empty) {
            MessageFrames::Empty => self.frames = MessageFrames::single(frame),
            MessageFrames::Single {
                frame: existing, ..
            } => {
                let mut frames = VecDeque::with_capacity(2);
                frames.push_back(frame);
                frames.push_back(existing);
                self.frames = MessageFrames::Multi(frames);
            }
            MessageFrames::Multi(mut frames) => {
                frames.push_front(frame);
                self.frames = MessageFrames::Multi(frames);
            }
        }
    }

    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, Bytes> {
        self.frames.public_iter()
    }

    pub(crate) fn frame_iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = &Bytes> + ExactSizeIterator + FusedIterator + Clone + '_
    {
        IterRef {
            inner: match &self.frames {
                MessageFrames::Empty => IterInner::Empty,
                MessageFrames::Single { frame, .. } => IterInner::Single(Some(frame)),
                MessageFrames::Multi(frames) => IterInner::Multi(frames.iter()),
            },
        }
    }

    pub(crate) fn pop_front(&mut self) -> Option<Bytes> {
        match &mut self.frames {
            MessageFrames::Empty => None,
            MessageFrames::Single { .. } => {
                match std::mem::replace(&mut self.frames, MessageFrames::Empty) {
                    MessageFrames::Single { frame, .. } => Some(frame),
                    _ => unreachable!("variant checked"),
                }
            }
            MessageFrames::Multi(frames) => {
                let frame = frames.pop_front();
                self.frames.normalize();
                frame
            }
        }
    }

    pub fn len(&self) -> usize {
        match &self.frames {
            MessageFrames::Empty => 0,
            MessageFrames::Single { .. } => 1,
            MessageFrames::Multi(frames) => frames.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.frames, MessageFrames::Empty)
    }

    pub fn get(&self, index: usize) -> Option<&Bytes> {
        match &self.frames {
            MessageFrames::Single { frame, .. } if index == 0 => Some(frame),
            MessageFrames::Multi(frames) => frames.get(index),
            MessageFrames::Empty | MessageFrames::Single { .. } => None,
        }
    }

    pub fn into_vec(self) -> Vec<Bytes> {
        match self.frames {
            MessageFrames::Empty => Vec::new(),
            MessageFrames::Single { frame, .. } => vec![frame],
            MessageFrames::Multi(frames) => Vec::from(frames),
        }
    }

    pub fn into_vecdeque(self) -> VecDeque<Bytes> {
        match self.frames {
            MessageFrames::Empty => VecDeque::new(),
            MessageFrames::Single { frame, .. } => {
                let mut frames = VecDeque::with_capacity(1);
                frames.push_back(frame);
                frames
            }
            MessageFrames::Multi(frames) => frames,
        }
    }

    pub fn prepend(&mut self, message: &ZmqMessage) {
        for frame in message.frame_iter().rev() {
            self.push_front(frame.clone());
        }
    }

    pub fn split_off(&mut self, at: usize) -> ZmqMessage {
        let frames = match &mut self.frames {
            MessageFrames::Empty => {
                if at == 0 {
                    MessageFrames::Empty
                } else {
                    panic!("`at` out of bounds")
                }
            }
            MessageFrames::Single { .. } => match at {
                0 => std::mem::replace(&mut self.frames, MessageFrames::Empty),
                1 => MessageFrames::Empty,
                _ => panic!("`at` out of bounds"),
            },
            MessageFrames::Multi(frames) => MessageFrames::from_vecdeque(frames.split_off(at)),
        };
        self.frames.normalize();
        ZmqMessage { frames }
    }
}

impl TryFrom<Vec<Bytes>> for ZmqMessage {
    type Error = ZmqEmptyMessageError;
    fn try_from(v: Vec<Bytes>) -> Result<Self, Self::Error> {
        if v.is_empty() {
            Err(ZmqEmptyMessageError)
        } else {
            Ok(Self {
                frames: MessageFrames::from_vec(v),
            })
        }
    }
}

impl TryFrom<VecDeque<Bytes>> for ZmqMessage {
    type Error = ZmqEmptyMessageError;
    fn try_from(v: VecDeque<Bytes>) -> Result<Self, Self::Error> {
        if v.is_empty() {
            Err(ZmqEmptyMessageError)
        } else {
            Ok(Self {
                frames: MessageFrames::from_vecdeque(v),
            })
        }
    }
}

impl From<Vec<u8>> for ZmqMessage {
    fn from(v: Vec<u8>) -> Self {
        ZmqMessage::from(Bytes::from(v))
    }
}

impl From<Bytes> for ZmqMessage {
    fn from(b: Bytes) -> Self {
        Self {
            frames: MessageFrames::single(b),
        }
    }
}

impl From<String> for ZmqMessage {
    fn from(s: String) -> Self {
        let b: Bytes = s.into();
        ZmqMessage::from(b)
    }
}

impl From<&str> for ZmqMessage {
    fn from(s: &str) -> Self {
        ZmqMessage::from(s.to_owned())
    }
}

impl TryFrom<ZmqMessage> for Vec<u8> {
    type Error = &'static str;

    fn try_from(z: ZmqMessage) -> Result<Self, Self::Error> {
        if z.len() != 1 {
            return Err("Message must have only 1 frame to convert to Vec<u8>");
        }
        Ok(z.into_vecdeque().pop_front().unwrap().to_vec())
    }
}

impl TryFrom<ZmqMessage> for String {
    type Error = &'static str;

    fn try_from(z: ZmqMessage) -> Result<Self, Self::Error> {
        if z.len() != 1 {
            return Err("Message must have only 1 frame to convert to String");
        }
        match String::from_utf8(z.into_vecdeque().pop_front().unwrap().to_vec()) {
            Ok(s) => Ok(s),
            Err(_) => Err("Could not parse string from message"),
        }
    }
}
