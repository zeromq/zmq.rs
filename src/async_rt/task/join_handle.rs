#[cfg(feature = "async-std-runtime")]
use async_std::task as rt_task;
#[cfg(feature = "tokio-runtime")]
use tokio::task as rt_task;

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::JoinError;

pub struct JoinHandle<T>(rt_task::JoinHandle<T>);
impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // In async-std, the program aborts on panic so results arent returned. To
        // unify with tokio, we simply make an `Ok` result.
        let result = rt_task::JoinHandle::poll(Pin::new(&mut self.0), cx);
        #[cfg(feature = "async-std-runtime")]
        return result.map(Ok);
        #[cfg(feature = "tokio-runtime")]
        return result.map_err(|e| e.into());
    }
}
impl<T> From<rt_task::JoinHandle<T>> for JoinHandle<T> {
    fn from(h: rt_task::JoinHandle<T>) -> Self {
        Self(h)
    }
}
