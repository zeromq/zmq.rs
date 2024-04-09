#[cfg(feature = "async-std-runtime")]
use async_std::task as rt_task;
#[cfg(feature = "smol-runtime")]
use smol::Task as SmolTask;
#[cfg(feature = "tokio-runtime")]
use tokio::task as rt_task;

pub struct JoinHandle<T>(pub(crate) JoinHandleImpl<T>);

use super::JoinError;

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

#[cfg(feature = "smol-runtime")]
type JoinHandleImpl<T> = SmolTask<T>;

impl<T> JoinHandle<T> {
    pub(crate) fn new(handle: JoinHandleImpl<T>) -> Self {
        JoinHandle(handle)
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, ()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}
