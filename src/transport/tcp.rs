#[cfg(feature = "tokio-runtime")]
use tokio::net::{TcpListener, TcpStream};

#[cfg(any(feature = "async-std-runtime", feature = "async-dispatcher-runtime"))]
use async_std::net::{TcpListener, TcpStream};

use super::make_framed;
use super::AcceptStopHandle;
use crate::async_rt;
use crate::codec::FramedIo;
use crate::endpoint::{Endpoint, Host, Port};
use crate::task_handle::TaskHandle;
use crate::ZmqResult;

use futures::{select, FutureExt};

#[cfg(feature = "tokio-runtime")]
const TCP_SOCKET_BUFFER_SIZE: usize = 4 * 1024 * 1024;

pub(crate) async fn connect(host: &Host, port: Port) -> ZmqResult<(FramedIo, Endpoint)> {
    let raw_socket = TcpStream::connect((host.to_string().as_str(), port)).await?;
    // For some reason set_nodelay doesn't work on windows. See
    // https://github.com/zeromq/zmq.rs/issues/148 for details
    #[cfg(not(windows))]
    raw_socket.set_nodelay(true)?;
    tune_socket_buffers(&raw_socket);
    let peer_addr = raw_socket.peer_addr()?;

    Ok((make_framed(raw_socket), Endpoint::from_tcp_addr(peer_addr)))
}

pub(crate) async fn begin_accept<T>(
    host: Host,
    port: Port,
    cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind((host.to_string().as_str(), port)).await?;
    let resolved_addr = listener.local_addr()?;
    debug_assert_ne!(resolved_addr.port(), 0);
    let endpoint = Endpoint::Tcp(host, resolved_addr.port());
    begin_accept_bound(listener, endpoint, cback).await
}

pub(crate) async fn begin_accept_listener<T>(
    listener: std::net::TcpListener,
    cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    let resolved_addr = listener.local_addr()?;
    listener.set_nonblocking(true)?;

    #[cfg(feature = "tokio-runtime")]
    let listener = TcpListener::from_std(listener)?;
    #[cfg(any(feature = "async-std-runtime", feature = "async-dispatcher-runtime"))]
    let listener = TcpListener::from(listener);

    begin_accept_bound(listener, Endpoint::from_tcp_addr(resolved_addr), cback).await
}

async fn begin_accept_bound<T>(
    listener: TcpListener,
    endpoint: Endpoint,
    cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopHandle)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    let (stop_channel, stop_callback) = futures::channel::oneshot::channel::<()>();
    let task_handle = async_rt::task::spawn(async move {
        let mut stop_callback = stop_callback.fuse();
        loop {
            select! {
                incoming = listener.accept().fuse() => {
                    let maybe_accepted: Result<_, _> = incoming
                        .and_then(|(raw_socket, remote_addr)| {
                            raw_socket
                                .set_nodelay(true)
                                .map(|_| {
                                    tune_socket_buffers(&raw_socket);
                                    (raw_socket, remote_addr)
                                })
                        })
                        .map(|(raw_socket, remote_addr)| {
                            (
                                make_framed(raw_socket),
                                Endpoint::from_tcp_addr(remote_addr),
                            )
                        })
                        .map_err(|err| err.into());
                    async_rt::task::spawn(cback(maybe_accepted));
                }
                _ = stop_callback => {
                    break
                }
            }
        }
        Ok(())
    });
    Ok((
        endpoint,
        AcceptStopHandle(TaskHandle::new(stop_channel, task_handle)),
    ))
}

#[cfg(feature = "tokio-runtime")]
fn tune_socket_buffers(raw_socket: &TcpStream) {
    let socket = socket2::SockRef::from(raw_socket);
    if let Err(error) = socket.set_recv_buffer_size(TCP_SOCKET_BUFFER_SIZE) {
        log::debug!("failed to set TCP receive buffer size: {error}");
    }
    if let Err(error) = socket.set_send_buffer_size(TCP_SOCKET_BUFFER_SIZE) {
        log::debug!("failed to set TCP send buffer size: {error}");
    }
}

#[cfg(not(feature = "tokio-runtime"))]
fn tune_socket_buffers(_raw_socket: &TcpStream) {}
