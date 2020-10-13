// TODO: Conditionally compile things
mod tokio;

use self::tokio as tk;
use crate::codec::FramedIo;
use crate::endpoint::{Endpoint, Host, Port};
use crate::transport::AcceptStopChannel;
use crate::ZmqResult;

pub(crate) async fn connect(host: Host, port: Port) -> ZmqResult<(FramedIo, Endpoint)> {
    tk::connect(host, port).await
}

pub(crate) async fn begin_accept<T>(
    host: Host,
    port: Port,
    cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopChannel)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    tk::begin_accept(host, port, cback).await
}
