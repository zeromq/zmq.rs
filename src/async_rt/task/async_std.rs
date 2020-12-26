use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::JoinError;

pub fn spawn<T>(task: T) -> JoinHandle<T::Output>
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    async_std::task::spawn(task).into()
}

pub struct JoinHandle<T>(async_std::task::JoinHandle<T>);
impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // In async-std, the program aborts on panic so results arent returned. To
        // unify with tokio, we simply make an `Ok` result.
        async_std::task::JoinHandle::poll(Pin::new(&mut self.0), cx).map(|p| Ok(p))
    }
}
impl<T> From<async_std::task::JoinHandle<T>> for JoinHandle<T> {
    fn from(h: async_std::task::JoinHandle<T>) -> Self {
        Self(h)
    }
}
