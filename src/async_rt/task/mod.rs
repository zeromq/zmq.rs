#[cfg(feature = "tokio-runtime")]
mod tokio;
#[cfg(feature = "tokio-runtime")]
use self::tokio as rt;

#[cfg(feature = "async-std-runtime")]
mod async_std;
#[cfg(feature = "async-std-runtime")]
use self::async_std as rt;

use std::any::Any;

pub use rt::{spawn, JoinHandle};

/// The type of error the occurred in the task. See [`JoinHandle`].
///
/// Note that some async runtimes (like async-std), may not bubble up panics
/// but instead abort the entire application. In these runtimes, you won't ever
/// get the opportunity to see the JoinError, because you're already dead.
pub enum JoinError {
    Cancelled,
    Panic(Box<dyn Any + Send + 'static>),
}
impl JoinError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub fn is_panic(&self) -> bool {
        !self.is_cancelled()
    }
}
