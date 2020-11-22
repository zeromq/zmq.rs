//! Tokio-specific implementations

use super::AcceptStopHandle;
use crate::codec::FramedIo;
use crate::endpoint::{Endpoint, Host, Port};
use crate::task_handle::TaskHandle;
use crate::ZmqResult;

use futures::{select, FutureExt};
use tokio_util::compat::Tokio02AsyncReadCompatExt;
use tokio_util::compat::Tokio02AsyncWriteCompatExt;

pub(crate) async fn connect(host: Host, port: Port) -> ZmqResult<(FramedIo, Endpoint)> {
    let raw_socket = tokio::net::TcpStream::connect((host.to_string().as_str(), port)).await?;
    let peer_addr = raw_socket.peer_addr()?;
    let (read, write) = tokio::io::split(raw_socket);
    let raw_sock = FramedIo::new(Box::new(read.compat()), Box::new(write.compat_write()));
    Ok((raw_sock, Endpoint::from_tcp_addr(peer_addr)))
}

pub(crate) async fn begin_accept<T>(
    mut host: Host,
    port: Port,
    cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    let mut listener = tokio::net::TcpListener::bind((host.to_string().as_str(), port)).await?;
    let resolved_addr = listener.local_addr()?;
    let (stop_channel, stop_callback) = futures::channel::oneshot::channel::<()>();
    let task_handle = tokio::spawn(async move {
        let mut stop_callback = stop_callback.fuse();
        loop {
            select! {
                incoming = listener.accept().fuse() => {
                    let maybe_accepted: Result<_, _> = incoming.map(|(raw_sock, remote_addr)| {
                        let (read, write) = tokio::io::split(raw_sock);
                        let raw_sock = FramedIo::new(Box::new(read.compat()), Box::new(write.compat_write()));
                        (raw_sock, Endpoint::from_tcp_addr(remote_addr))
                    }).map_err(|err| err.into());
                    tokio::spawn(cback(maybe_accepted));
                },
                _ = stop_callback => {
                    break
                }
            }
        }
        Ok(())
    });
    debug_assert_ne!(resolved_addr.port(), 0);
    let port = resolved_addr.port();
    let resolved_host: Host = resolved_addr.ip().into();
    if let Host::Ipv4(ip) = host {
        debug_assert_eq!(ip, resolved_addr.ip());
        host = resolved_host;
    } else if let Host::Ipv6(ip) = host {
        debug_assert_eq!(ip, resolved_addr.ip());
        host = resolved_host;
    }
    Ok((
        Endpoint::Tcp(host, port),
        AcceptStopHandle(TaskHandle::new(stop_channel, task_handle)),
    ))
}
