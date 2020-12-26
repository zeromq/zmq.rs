#[cfg(feature = "tokio-runtime")]
mod tokio;
#[cfg(feature = "tokio-runtime")]
use self::tokio as rt;

#[cfg(feature = "async-std-runtime")]
mod async_std;
#[cfg(feature = "async-std-runtime")]
use self::async_std as rt;

use super::AcceptStopHandle;
use crate::codec::FramedIo;
use crate::endpoint::{Endpoint, Host, Port};
use crate::ZmqResult;

pub(crate) async fn connect(host: Host, port: Port) -> ZmqResult<(FramedIo, Endpoint)> {
    rt::connect(host, port).await
}

pub(crate) async fn begin_accept<T>(
    host: Host,
    port: Port,
    cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    rt::begin_accept(host, port, cback).await
}
