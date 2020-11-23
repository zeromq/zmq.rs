//! General types and traits to facilitate compatibility across async runtimes

use crate::codec::ZmqCodec;
use futures_codec::{FramedRead, FramedWrite};

// Enables us to have multiple bounds on the dyn trait in `InnerFramed`
pub trait FrameableRead: futures::AsyncRead + Unpin + Send + Sync {}
impl<T> FrameableRead for T where T: futures::AsyncRead + Unpin + Send + Sync {}
pub trait FrameableWrite: futures::AsyncWrite + Unpin + Send + Sync {}
impl<T> FrameableWrite for T where T: futures::AsyncWrite + Unpin + Send + Sync {}

/// Equivalent to [`futures_codec::Framed<T, ZmqCodec>`] or
/// [`tokio_util::codec::Framed`]
pub(crate) struct FramedIo {
    pub read_half: futures_codec::FramedRead<Box<dyn FrameableRead>, ZmqCodec>,
    pub write_half: futures_codec::FramedWrite<Box<dyn FrameableWrite>, ZmqCodec>,
}

impl FramedIo {
    pub fn new(read_half: Box<dyn FrameableRead>, write_half: Box<dyn FrameableWrite>) -> Self {
        let read_half = FramedRead::new(read_half, ZmqCodec::new());
        let write_half = FramedWrite::new(write_half, ZmqCodec::new());
        Self {
            read_half,
            write_half,
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        futures_codec::FramedRead<Box<dyn FrameableRead>, ZmqCodec>,
        futures_codec::FramedWrite<Box<dyn FrameableWrite>, ZmqCodec>,
    ) {
        (self.read_half, self.write_half)
    }
}
