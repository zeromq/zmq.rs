mod tokio;
use std::any::Any;

use self::tokio as rt;

pub use rt::{spawn, spawn_blocking, JoinHandle};

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
        if let Self::Cancelled = self {
            true
        } else {
            false
        }
    }

    pub fn is_panic(&self) -> bool {
        !self.is_cancelled()
    }
}
