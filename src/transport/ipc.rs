#[cfg(all(feature = "tokio-runtime", target_family = "unix"))]
use tokio::net::{UnixListener, UnixStream};

#[cfg(all(
    any(feature = "async-std-runtime", feature = "async-dispatcher-runtime"),
    target_family = "unix"
))]
use async_std::os::unix::net::{UnixListener, UnixStream};

#[cfg(windows)]
use win_uds::net::{AsyncListener as UnixListener, AsyncStream as UnixStream};

#[cfg(target_family = "unix")]
use super::make_framed;
use super::AcceptStopHandle;
use crate::async_rt;
use crate::codec::FramedIo;
use crate::endpoint::Endpoint;
use crate::task_handle::TaskHandle;
use crate::ZmqResult;

use futures::channel::oneshot;
use futures::{select, FutureExt};

use std::path::Path;

#[cfg(target_family = "unix")]
fn pathname_from_unix_addr(addr: impl UnixSocketAddrExt) -> Option<std::path::PathBuf> {
    addr.as_pathname().map(|a| a.to_owned())
}

#[cfg(target_family = "unix")]
trait UnixSocketAddrExt {
    fn as_pathname(&self) -> Option<&Path>;
}

#[cfg(all(feature = "tokio-runtime", target_family = "unix"))]
impl UnixSocketAddrExt for tokio::net::unix::SocketAddr {
    fn as_pathname(&self) -> Option<&Path> {
        self.as_pathname()
    }
}

#[cfg(all(
    any(feature = "async-std-runtime", feature = "async-dispatcher-runtime"),
    target_family = "unix"
))]
impl UnixSocketAddrExt for async_std::os::unix::net::SocketAddr {
    fn as_pathname(&self) -> Option<&Path> {
        self.as_pathname()
    }
}

#[cfg(windows)]
fn make_framed(stream: UnixStream) -> FramedIo {
    use futures::AsyncReadExt;
    let (read, write) = stream.split();
    FramedIo::new(Box::new(read), Box::new(write))
}

pub(crate) async fn connect(path: &Path) -> ZmqResult<(FramedIo, Endpoint)> {
    let raw_socket = UnixStream::connect(path).await?;

    #[cfg(target_family = "unix")]
    let peer_addr = raw_socket.peer_addr()?;
    #[cfg(target_family = "unix")]
    let peer_addr = pathname_from_unix_addr(peer_addr);

    #[cfg(windows)]
    let peer_addr = Some(path.to_owned());

    Ok((make_framed(raw_socket), Endpoint::Ipc(peer_addr)))
}

pub(crate) async fn begin_accept<T>(
    path: &Path,
    cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    let wildcard: &Path = "*".as_ref();
    if path == wildcard {
        todo!("Need to implement support for wildcard paths!");
    }

    #[cfg(all(feature = "tokio-runtime", target_family = "unix"))]
    let listener = UnixListener::bind(path)?;
    #[cfg(all(
        any(feature = "async-std-runtime", feature = "async-dispatcher-runtime"),
        target_family = "unix"
    ))]
    let listener = UnixListener::bind(path).await?;
    #[cfg(windows)]
    let listener = UnixListener::bind(path)?;

    #[cfg(target_family = "unix")]
    let resolved_addr = listener.local_addr()?;
    #[cfg(target_family = "unix")]
    let resolved_addr = pathname_from_unix_addr(resolved_addr);

    #[cfg(windows)]
    let resolved_addr = Some(path.to_owned());

    let listener_addr = resolved_addr.clone();
    let (stop_channel, stop_callback) = oneshot::channel::<()>();
    let task_handle = async_rt::task::spawn(async move {
        let mut stop_callback = stop_callback.fuse();
        loop {
            select! {
                incoming = listener.accept().fuse() => {
                    let maybe_accepted: Result<_, _> = incoming.map(|(raw_socket, peer_addr)| {
                        #[cfg(target_family = "unix")]
                        let peer_addr = pathname_from_unix_addr(peer_addr);
                        #[cfg(windows)]
                        let peer_addr = {
                            let _ = peer_addr;
                            None
                        };
                        (make_framed(raw_socket), Endpoint::Ipc(peer_addr))
                    }).map_err(|err| err.into());
                    async_rt::task::spawn(cback(maybe_accepted));
                },
                _ = stop_callback => {
                    log::debug!("Accept task received stop signal. {:?}", listener_addr);
                    break
                }
            }
        }
        drop(listener);
        if let Some(listener_addr) = listener_addr {
            #[cfg(any(
                all(
                    any(feature = "async-std-runtime", feature = "async-dispatcher-runtime"),
                    target_family = "unix"
                ),
                all(
                    any(feature = "async-std-runtime", feature = "async-dispatcher-runtime"),
                    windows
                )
            ))]
            use async_std::fs::remove_file;
            #[cfg(all(
                feature = "tokio-runtime",
                any(target_family = "unix", windows),
                not(any(feature = "async-std-runtime", feature = "async-dispatcher-runtime"))
            ))]
            use tokio::fs::remove_file;

            if let Err(err) = remove_file(&listener_addr).await {
                log::warn!(
                    "Could not delete unix socket at {}: {}",
                    listener_addr.display(),
                    err
                );
            }
        }
        Ok(())
    });
    Ok((
        Endpoint::Ipc(resolved_addr),
        AcceptStopHandle(TaskHandle::new(stop_channel, task_handle)),
    ))
}
