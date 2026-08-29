//! `inproc://` transport using [`crate::Context`] rendezvous and message channels.

use super::AcceptStopHandle;
use crate::async_rt;
use crate::context::Context;
use crate::endpoint::Endpoint;
use crate::peer_io::{inproc_peer_pair, PeerIo};
use crate::task_handle::TaskHandle;
use crate::{ZmqError, ZmqResult};

use futures::channel::{mpsc, oneshot};
use futures::{select, FutureExt, StreamExt};

use std::io;

pub(crate) async fn connect(name: &str, context: &Context) -> ZmqResult<(PeerIo, Endpoint)> {
    let accept_tx = context.inproc_listener(name).ok_or_else(|| {
        ZmqError::Network(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "inproc endpoint not bound",
        ))
    })?;

    let (bind_end, connect_end) = inproc_peer_pair();
    accept_tx
        .unbounded_send(bind_end)
        .map_err(|_send_error| ZmqError::Socket("inproc bind endpoint was closed"))?;

    Ok((connect_end, Endpoint::Inproc(name.to_string())))
}

pub(crate) async fn begin_accept<T>(
    name: String,
    context: Context,
    cback: impl Fn(ZmqResult<(PeerIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    let (accept_tx, mut accept_rx) = mpsc::unbounded();
    context.register_inproc_listener(&name, accept_tx)?;

    let endpoint = Endpoint::Inproc(name.clone());
    let (stop_channel, stop_callback) = oneshot::channel::<()>();
    let context_for_task = context.clone();
    let name_for_task = name.clone();
    let endpoint_for_peer = endpoint.clone();

    let task_handle = async_rt::task::spawn(async move {
        let mut stop_callback = stop_callback.fuse();
        loop {
            select! {
                incoming = accept_rx.next() => {
                    match incoming {
                        Some(io) => {
                            let peer_endpoint = endpoint_for_peer.clone();
                            async_rt::task::spawn(cback(Ok((io, peer_endpoint))));
                        }
                        None => break,
                    }
                }
                _ = stop_callback => {
                    log::debug!("inproc accept task received stop signal for {name_for_task}");
                    break;
                }
            }
        }
        let _ = context_for_task.unregister_inproc(&name_for_task);
        Ok(())
    });

    Ok((
        endpoint,
        AcceptStopHandle(TaskHandle::new(stop_channel, task_handle)),
    ))
}
