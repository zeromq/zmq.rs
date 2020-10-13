//! General types and traits to facilitate compatibility across async runtimes

use crate::codec::ZmqCodec;

// We use dynamic dispatch to avoid complicated generics and simplify things
type Inner = futures_codec::Framed<Box<dyn Frameable>, ZmqCodec>;

// Enables us to have multiple bounds on the dyn trait in `InnerFramed`
pub trait Frameable: futures::AsyncWrite + futures::AsyncRead + Unpin + Send {}
impl<T> Frameable for T where T: futures::AsyncWrite + futures::AsyncRead + Unpin + Send {}

/// Equivalent to [`futures_codec::Framed<T, ZmqCodec>`] or
/// [`tokio_util::codec::Framed`]
pub(crate) struct FramedIo(Inner);
impl FramedIo {
    pub fn new(frameable: Box<dyn Frameable>) -> Self {
        let inner = futures_codec::Framed::new(frameable, ZmqCodec::new());
        Self(inner)
    }
}

impl std::ops::Deref for FramedIo {
    type Target = Inner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for FramedIo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
