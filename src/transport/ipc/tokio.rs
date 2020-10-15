//! Tokio-specific implementations

use crate::codec::FramedIo;
use crate::endpoint::Endpoint;
use crate::transport::AcceptStopChannel;
use crate::ZmqResult;

use futures::{select, FutureExt};
use std::path::{Path, PathBuf};
use tokio_util::compat::Tokio02AsyncReadCompatExt;

pub(crate) async fn connect(path: PathBuf) -> ZmqResult<(FramedIo, Endpoint)> {
    let raw_socket = tokio::net::UnixStream::connect(&path).await?;
    let peer_addr = raw_socket.peer_addr()?;
    let peer_addr = peer_addr.as_pathname().map(|a| a.to_owned());
    let boxed_sock = Box::new(raw_socket.compat());
    Ok((FramedIo::new(boxed_sock), Endpoint::Ipc(peer_addr)))
}

pub(crate) async fn begin_accept<T>(
    path: PathBuf,
    cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopChannel)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    let wildcard: &Path = "*".as_ref();
    if path == wildcard {
        todo!("Need to implement support for wildcard paths!");
    }
    let mut listener = tokio::net::UnixListener::bind(path)?;
    let resolved_addr = listener.local_addr()?;
    let resolved_addr = resolved_addr.as_pathname().map(|a| a.to_owned());
    let listener_addr = resolved_addr.clone();
    let (stop_handle, stop_callback) = futures::channel::oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut stop_callback = stop_callback.fuse();
        loop {
            select! {
                incoming = listener.accept().fuse() => {
                    let maybe_accepted: Result<_, _> = incoming.map(|(raw_sock, peer_addr)| {
                        let raw_sock = FramedIo::new(Box::new(raw_sock.compat()));
                        let peer_addr = peer_addr.as_pathname().map(|a| a.to_owned());
                        (raw_sock, Endpoint::Ipc(peer_addr))
                    }).map_err(|err| err.into());
                    tokio::spawn(cback(maybe_accepted.into()));
                },
                _ = stop_callback => {
                    log::debug!("Accept task received stop signal. {:?}", listener_addr);
                    break
                }
            }
        }
        drop(listener);
        if let Some(listener_addr) = listener_addr {
            if let Err(err) = tokio::fs::remove_file(&listener_addr).await {
                log::warn!(
                    "Could not delete unix socket at {}: {}",
                    listener_addr.display(),
                    err
                );
            }
        }
    });
    Ok((Endpoint::Ipc(resolved_addr), AcceptStopChannel(stop_handle)))
}
