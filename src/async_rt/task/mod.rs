mod join_handle;

use std::any::Any;

pub use join_handle::JoinHandle;
use std::future::Future;

pub fn spawn<T>(task: T) -> JoinHandle<T::Output>
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    #[cfg(feature = "tokio-runtime")]
    let result = tokio::task::spawn(task).into();
    #[cfg(feature = "async-std-runtime")]
    let result = async_std::task::spawn(task).into();

    result
}

/// The type of error the occurred in the task. See [`JoinHandle`].
///
/// Note that some async runtimes (like async-std), may not bubble up panics
/// but instead abort the entire application. In these runtimes, you won't ever
/// get the opportunity to see the JoinError, because you're already dead.
#[derive(Debug)]
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

#[cfg(feature = "tokio-runtime")]
impl From<tokio::task::JoinError> for JoinError {
    fn from(err: tokio::task::JoinError) -> Self {
        if err.is_cancelled() {
            Self::Cancelled
        } else {
            Self::Panic(err.into_panic())
        }
    }
}

pub async fn sleep(duration: std::time::Duration) {
    #[cfg(feature = "tokio-runtime")]
    ::tokio::time::sleep(duration).await;
    #[cfg(feature = "async-std-runtime")]
    ::async_std::task::sleep(duration).await
}
