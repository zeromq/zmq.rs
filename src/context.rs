//! Process-local shared state for transports that need rendezvous.
//!
//! TCP and IPC sockets do not require a [`Context`]. `inproc://` bind/connect
//! use one shared [`Context`] so peers can find each other by name.

use crate::peer_io::PeerIo;
use crate::{ZmqError, ZmqResult};

use futures::channel::mpsc;
use parking_lot::Mutex;

use std::collections::HashMap;
use std::sync::Arc;

/// Sender used by `inproc` connect to deliver a peer to the bind side.
pub(crate) type InprocAcceptSender = mpsc::UnboundedSender<PeerIo>;

/// Shared context for in-process endpoint rendezvous.
///
/// Cloning a `Context` shares the same registry (`Arc`). Sockets that use
/// `inproc://` must be created with the same `Context` instance (or clones of it)
/// via [`crate::SocketOptions::context`].
#[derive(Clone, Debug, Default)]
pub struct Context {
    inner: Arc<ContextInner>,
}

#[derive(Debug, Default)]
struct ContextInner {
    /// Bound inproc listeners keyed by endpoint name.
    inproc: Mutex<HashMap<String, InprocBinding>>,
}

struct InprocBinding {
    accept_tx: InprocAcceptSender,
}

impl std::fmt::Debug for InprocBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InprocBinding").finish_non_exhaustive()
    }
}

impl Context {
    /// Creates a new empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an inproc listener for `name`.
    ///
    /// # Errors
    /// - Empty names are rejected.
    /// - Returns an error if `name` is already bound (libzmq `EADDRINUSE` analogue).
    pub(crate) fn register_inproc_listener(
        &self,
        name: &str,
        accept_tx: InprocAcceptSender,
    ) -> ZmqResult<()> {
        if name.is_empty() {
            return Err(ZmqError::Socket("inproc endpoint name must not be empty"));
        }
        let mut map = self.inner.inproc.lock();
        if map.contains_key(name) {
            return Err(ZmqError::Socket("Address already in use"));
        }
        map.insert(name.to_string(), InprocBinding { accept_tx });
        Ok(())
    }

    /// Removes a previously bound inproc listener.
    ///
    /// # Errors
    /// Returns an error if `name` is not currently bound in this context.
    pub(crate) fn unregister_inproc(&self, name: &str) -> ZmqResult<()> {
        let mut map = self.inner.inproc.lock();
        if map.remove(name).is_none() {
            return Err(ZmqError::Socket("No such inproc endpoint"));
        }
        Ok(())
    }

    /// Returns a clone of the accept sender for a bound inproc name, if any.
    pub(crate) fn inproc_listener(&self, name: &str) -> Option<InprocAcceptSender> {
        self.inner
            .inproc
            .lock()
            .get(name)
            .map(|binding| binding.accept_tx.clone())
    }

    /// Returns whether `name` is currently bound in this context.
    #[cfg(test)]
    pub(crate) fn inproc_is_bound(&self, name: &str) -> bool {
        self.inner.inproc.lock().contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_listener() -> InprocAcceptSender {
        let (tx, _rx) = mpsc::unbounded();
        tx
    }

    #[test]
    fn register_and_unregister_inproc_name() {
        let ctx = Context::new();
        assert!(!ctx.inproc_is_bound("step2"));

        ctx.register_inproc_listener("step2", dummy_listener())
            .unwrap();
        assert!(ctx.inproc_is_bound("step2"));
        assert!(ctx.inproc_listener("step2").is_some());

        ctx.unregister_inproc("step2").unwrap();
        assert!(!ctx.inproc_is_bound("step2"));
        assert!(ctx.inproc_listener("step2").is_none());
    }

    #[test]
    fn clones_share_the_same_registry() {
        let ctx = Context::new();
        let clone = ctx.clone();

        ctx.register_inproc_listener("shared", dummy_listener())
            .unwrap();
        assert!(clone.inproc_is_bound("shared"));

        clone.unregister_inproc("shared").unwrap();
        assert!(!ctx.inproc_is_bound("shared"));
    }

    #[test]
    fn double_bind_is_rejected() {
        let ctx = Context::new();
        ctx.register_inproc_listener("dup", dummy_listener())
            .unwrap();
        let err = ctx
            .register_inproc_listener("dup", dummy_listener())
            .unwrap_err();
        assert!(matches!(err, ZmqError::Socket("Address already in use")));
    }

    #[test]
    fn empty_name_is_rejected() {
        let ctx = Context::new();
        let err = ctx
            .register_inproc_listener("", dummy_listener())
            .unwrap_err();
        assert!(matches!(
            err,
            ZmqError::Socket("inproc endpoint name must not be empty")
        ));
    }

    #[test]
    fn unregister_missing_name_errors() {
        let ctx = Context::new();
        let err = ctx.unregister_inproc("missing").unwrap_err();
        assert!(matches!(err, ZmqError::Socket("No such inproc endpoint")));
    }

    #[test]
    fn separate_contexts_do_not_share_names() {
        let a = Context::new();
        let b = Context::new();
        a.register_inproc_listener("only-a", dummy_listener())
            .unwrap();
        assert!(!b.inproc_is_bound("only-a"));
        b.register_inproc_listener("only-a", dummy_listener())
            .unwrap();
    }
}
