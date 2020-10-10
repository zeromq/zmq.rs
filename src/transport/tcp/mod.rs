// TODO: Conditionally compile things
mod tokio;

use self::tokio as tk;
use crate::compat::FramedIo;
use crate::endpoint::{Endpoint, Host, Port};
use crate::ZmqResult;

use std::net::SocketAddr;

pub(crate) async fn connect(host: Host, port: Port) -> ZmqResult<(FramedIo, SocketAddr)> {
    tk::connect(host, port).await
}

/// Spawns an async task that listens for tcp connections at the provided
/// address.
///
/// `cback` will be invoked when a tcp connection is accepted. If the result was
/// `Ok`, we get a tuple containing the framed raw socket, along with the ip
/// address of the remote connection accepted.
pub(crate) async fn begin_accept<T>(
    host: Host,
    port: Port,
    cback: impl Fn(ZmqResult<(FramedIo, SocketAddr)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, futures::channel::oneshot::Sender<bool>)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    tk::begin_accept(host, port, cback).await
}
