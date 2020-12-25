use super::JoinError;

use std::{future::Future, task::Context};
use std::{pin::Pin, task::Poll};

pub fn spawn<T>(task: T) -> JoinHandle<T::Output>
where
    T: Future + Send + 'static,
    T::Output: Send + 'static,
{
    tokio::task::spawn(task).into()
}

pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f).into()
}

impl From<tokio::task::JoinError> for super::JoinError {
    fn from(err: tokio::task::JoinError) -> Self {
        if err.is_cancelled() {
            Self::Cancelled
        } else {
            Self::Panic(err.into_panic())
        }
    }
}

pub struct JoinHandle<T>(tokio::task::JoinHandle<T>);
impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        tokio::task::JoinHandle::poll(Pin::new(&mut self.0), cx).map_err(|e| e.into())
    }
}
impl<T> From<tokio::task::JoinHandle<T>> for JoinHandle<T> {
    fn from(h: tokio::task::JoinHandle<T>) -> Self {
        Self(h)
    }
}
