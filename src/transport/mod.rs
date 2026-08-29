mod inproc;
#[cfg(all(feature = "ipc-transport", any(target_family = "unix", windows)))]
mod ipc;
#[cfg(feature = "tcp-transport")]
mod tcp;

use crate::codec::FramedIo;
use crate::context::Context;
use crate::endpoint::Endpoint;
use crate::peer_io::PeerIo;
use crate::task_handle::TaskHandle;
use crate::{ZmqError, ZmqResult};

/// An already-bound transport listener that a `ZeroMQ` socket can adopt.
#[derive(Debug)]
#[non_exhaustive]
pub enum Listener {
    /// A TCP listener bound by the caller.
    #[cfg(feature = "tcp-transport")]
    Tcp(std::net::TcpListener),
    /// A Unix-domain listener bound by the caller.
    #[cfg(all(feature = "ipc-transport", target_family = "unix"))]
    Ipc(std::os::unix::net::UnixListener),
}

#[cfg(feature = "tcp-transport")]
impl From<std::net::TcpListener> for Listener {
    fn from(listener: std::net::TcpListener) -> Self {
        Self::Tcp(listener)
    }
}

#[cfg(all(feature = "ipc-transport", target_family = "unix"))]
impl From<std::os::unix::net::UnixListener> for Listener {
    fn from(listener: std::os::unix::net::UnixListener) -> Self {
        Self::Ipc(listener)
    }
}

macro_rules! do_if_enabled {
    ($feature:literal, $body:expr) => {{
        #[cfg(feature = $feature)]
        {
            $body
        }

        #[cfg(not(feature = $feature))]
        panic!("feature \"{}\" is not enabled", $feature)
    }};
}

/// Connects to the given endpoint
///
/// # Panics
/// Panics if the requested endpoint uses a transport type that isn't enabled
pub(crate) async fn connect(
    endpoint: &Endpoint,
    context: Option<&Context>,
) -> ZmqResult<(PeerIo, Endpoint)> {
    match endpoint {
        Endpoint::Tcp(_host, _port) => {
            let (io, endpoint) =
                do_if_enabled!("tcp-transport", tcp::connect(_host, *_port).await)?;
            Ok((PeerIo::Framed(io), endpoint))
        }
        Endpoint::Ipc(_path) => {
            #[cfg(all(feature = "ipc-transport", any(target_family = "unix", windows)))]
            {
                let (io, endpoint) = if let Some(path) = _path {
                    ipc::connect(path).await?
                } else {
                    return Err(crate::error::ZmqError::Socket(
                        "Cannot connect to an unnamed ipc socket",
                    ));
                };
                Ok((PeerIo::Framed(io), endpoint))
            }
            #[cfg(not(all(feature = "ipc-transport", any(target_family = "unix", windows))))]
            panic!("IPC transport is not available on this platform")
        }
        Endpoint::Inproc(name) => {
            let context = context.ok_or(ZmqError::Socket(
                "inproc transport requires a Context; set it via SocketOptions::context",
            ))?;
            inproc::connect(name, context).await
        }
    }
}

pub struct AcceptStopHandle(pub(crate) TaskHandle<()>);

/// Spawns an async task that listens for connections at the provided endpoint.
///
/// `cback` will be invoked when a connection is accepted. If the result was
/// `Ok`, it will receive a tuple containing the peer I/O, along with
/// the endpoint of the remote connection accepted.
///
/// Returns a `ZmqResult`, which when Ok is a tuple of the resolved bound
/// endpoint, as well as a channel to stop the async accept task
///
/// # Panics
/// Panics if the requested endpoint uses a transport type that isn't enabled
pub(crate) async fn begin_accept<T>(
    endpoint: Endpoint,
    context: Option<Context>,
    cback: impl Fn(ZmqResult<(PeerIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    let upstream_cback = cback;
    match endpoint {
        Endpoint::Tcp(_host, _port) => {
            let cback = move |result: ZmqResult<(FramedIo, Endpoint)>| {
                let result = result.map(|(io, endpoint)| (PeerIo::Framed(io), endpoint));
                upstream_cback(result)
            };
            do_if_enabled!(
                "tcp-transport",
                tcp::begin_accept(_host, _port, cback).await
            )
        }
        Endpoint::Ipc(_path) => {
            #[cfg(all(feature = "ipc-transport", any(target_family = "unix", windows)))]
            {
                let cback = move |result: ZmqResult<(FramedIo, Endpoint)>| {
                    let result = result.map(|(io, endpoint)| (PeerIo::Framed(io), endpoint));
                    upstream_cback(result)
                };
                if let Some(path) = _path {
                    ipc::begin_accept(&path, cback).await
                } else {
                    Err(crate::error::ZmqError::Socket(
                        "Cannot begin accepting peers at an unnamed ipc socket",
                    ))
                }
            }
            #[cfg(not(all(feature = "ipc-transport", any(target_family = "unix", windows))))]
            panic!("IPC transport is not available on this platform")
        }
        Endpoint::Inproc(name) => {
            let context = context.ok_or(ZmqError::Socket(
                "inproc transport requires a Context; set it via SocketOptions::context",
            ))?;
            inproc::begin_accept(name, context, upstream_cback).await
        }
    }
}

pub(crate) async fn begin_accept_listener<T>(
    listener: Listener,
    upstream_cback: impl Fn(ZmqResult<(PeerIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    match listener {
        #[cfg(feature = "tcp-transport")]
        Listener::Tcp(listener) => {
            let cback = move |result: ZmqResult<(FramedIo, Endpoint)>| {
                let result = result.map(|(io, endpoint)| (PeerIo::Framed(io), endpoint));
                upstream_cback(result)
            };
            tcp::begin_accept_listener(listener, cback).await
        }
        #[cfg(all(feature = "ipc-transport", target_family = "unix"))]
        Listener::Ipc(listener) => {
            let cback = move |result: ZmqResult<(FramedIo, Endpoint)>| {
                let result = result.map(|(io, endpoint)| (PeerIo::Framed(io), endpoint));
                upstream_cback(result)
            };
            ipc::begin_accept_listener(listener, cback).await
        }
    }
}

#[allow(unused)]
#[cfg(feature = "tokio-runtime")]
fn make_framed<T>(stream: T) -> FramedIo
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Sync + 'static,
{
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
    let (read, write) = tokio::io::split(stream);
    FramedIo::new(Box::new(read.compat()), Box::new(write.compat_write()))
}

#[allow(unused)]
#[cfg(any(feature = "async-std-runtime", feature = "async-dispatcher-runtime"))]
fn make_framed<T>(stream: T) -> FramedIo
where
    T: futures::AsyncRead + futures::AsyncWrite + Send + Sync + 'static,
{
    use futures::AsyncReadExt;
    let (read, write) = stream.split();
    FramedIo::new(Box::new(read), Box::new(write))
}
