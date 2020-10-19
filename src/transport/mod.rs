mod ipc;
mod tcp;

use crate::codec::FramedIo;
use crate::endpoint::Endpoint;
use crate::error::ZmqError;
use crate::task_handle::TaskHandle;
use crate::ZmqResult;

pub(crate) async fn connect(endpoint: Endpoint) -> ZmqResult<(FramedIo, Endpoint)> {
    match endpoint {
        Endpoint::Tcp(host, port) => tcp::connect(host, port).await,
        Endpoint::Ipc(Some(path)) => ipc::connect(path).await,
        Endpoint::Ipc(None) => Err(ZmqError::Socket("Cannot connect to an unnamed ipc socket")),
    }
}

pub struct AcceptStopHandle(pub(crate) TaskHandle<()>);

/// Spawns an async task that listens for connections at the provided endpoint.
///
/// `cback` will be invoked when a connection is accepted. If the result was
/// `Ok`, it will receive a tuple containing the framed raw socket, along with
/// the endpoint of the remote connection accepted.
///
/// Returns a ZmqResult, which when Ok is a tuple of the resolved bound
/// endpoint, as well as a channel to stop the async accept task
pub(crate) async fn begin_accept<T>(
    endpoint: Endpoint,
    cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    match endpoint {
        Endpoint::Tcp(host, port) => tcp::begin_accept(host, port, cback).await,
        Endpoint::Ipc(Some(path)) => ipc::begin_accept(path, cback).await,
        Endpoint::Ipc(None) => Err(ZmqError::Socket(
            "Cannot begin accepting peers at an unnamed ipc socket",
        )),
    }
}
