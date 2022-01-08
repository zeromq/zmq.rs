// Helper functions to be runtime agnostic
use futures::Future;

#[cfg(feature = "tokio-runtime")]
extern crate tokio;
#[cfg(feature = "tokio-runtime")]
pub use tokio::{main, test};

#[cfg(feature = "async-std-runtime")]
extern crate async_std;
#[cfg(feature = "async-std-runtime")]
pub use async_std::{main, test};

#[allow(unused)]
#[cfg(feature = "tokio-runtime")]
pub async fn sleep(duration: std::time::Duration) {
    tokio::time::sleep(duration).await
}
#[allow(unused)]
#[cfg(feature = "async-std-runtime")]
pub async fn sleep(duration: std::time::Duration) {
    async_std::task::sleep(duration).await
}

#[allow(unused)]
#[cfg(feature = "tokio-runtime")]
pub fn spawn<T>(future: T)
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    tokio::spawn(future);
}

#[allow(unused)]
#[cfg(feature = "async-std-runtime")]
pub fn spawn<T>(future: T)
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    async_std::spawn(future);
}
