//! The small `AF_UNIX` adapter needed by the Windows IPC transport.
//!
//! `socket2` supplies the Winsock socket operations and `async-io` supplies
//! readiness notifications, independently of the selected task executor.

use async_io::Async;
use futures::{AsyncRead, AsyncWrite};
use socket2::{Domain, SockAddr, Socket, Type};
use std::io;
use std::net::Shutdown;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

pub(super) struct UnixStream(Async<Socket>);

impl UnixStream {
    pub(super) async fn connect(path: &Path) -> io::Result<Self> {
        let address = SockAddr::unix(path)?;
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        // Set nonblocking before connect: a full listen backlog must not block
        // the executor thread. Winsock reports WSAEWOULDBLOCK while connecting.
        socket.set_nonblocking(true)?;
        match socket.connect(&address) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
        let stream = Async::new_nonblocking(socket)?;
        stream.writable().await?;
        if let Some(error) = stream.get_ref().take_error()? {
            return Err(error);
        }
        Ok(Self(stream))
    }
}

impl AsyncRead for UnixStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // I/O through a shared reference cannot invalidate the registered socket.
        Pin::new(&mut &self.0).poll_read(cx, buffer)
    }
}

impl AsyncWrite for UnixStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &self.0).poll_write(cx, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut &self.0).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(self.0.get_ref().shutdown(Shutdown::Write))
    }
}

pub(super) struct UnixListener(Async<Socket>);

impl UnixListener {
    pub(super) fn bind(path: &Path) -> io::Result<Self> {
        let address = SockAddr::unix(path)?;
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        socket.bind(&address)?;
        socket.listen(128)?;
        Ok(Self(Async::new(socket)?))
    }

    pub(super) async fn accept(&self) -> io::Result<(UnixStream, SockAddr)> {
        let (socket, address) = self.0.read_with(Socket::accept).await?;
        // Accepted sockets do not necessarily inherit the listener's flags.
        Ok((UnixStream(Async::new(socket)?), address))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{AsyncReadExt, AsyncWriteExt, FutureExt};
    use std::path::PathBuf;
    use std::time::Duration;

    struct SocketPath(PathBuf);

    impl SocketPath {
        fn new() -> Self {
            // Short enough for sockaddr_un even with macOS's long temp prefix.
            Self(std::env::temp_dir().join(format!("zmq-{}", uuid::Uuid::new_v4().simple())))
        }
    }

    impl Drop for SocketPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    async fn bounded(test: impl std::future::Future<Output = ()>) {
        let timeout = async_io::Timer::after(Duration::from_secs(10));
        futures::pin_mut!(test, timeout);
        match futures::future::select(test, timeout).await {
            futures::future::Either::Left(_) => {}
            futures::future::Either::Right(_) => panic!("IPC adapter test timed out"),
        }
    }

    #[test]
    fn round_trip_with_backpressure_and_half_close() {
        async_io::block_on(bounded(async {
            let path = SocketPath::new();
            let listener = UnixListener::bind(&path.0).unwrap();
            let client = async {
                let mut stream = UnixStream::connect(&path.0).await.unwrap();
                stream.0.get_ref().set_send_buffer_size(4096).unwrap();
                stream.write_all(&vec![42; 1024 * 1024]).await.unwrap();
                stream.close().await.unwrap();
                let mut response = Vec::new();
                stream.read_to_end(&mut response).await.unwrap();
                assert_eq!(response, b"received");
            };
            let server = async {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut payload = Vec::new();
                stream.read_to_end(&mut payload).await.unwrap();
                assert_eq!(payload, vec![42; 1024 * 1024]);
                stream.write_all(b"received").await.unwrap();
                stream.close().await.unwrap();
            };
            futures::join!(client, server);
        }));
    }

    #[test]
    fn cancelled_accept_and_read_can_be_resumed() {
        async_io::block_on(bounded(async {
            let path = SocketPath::new();
            let listener = UnixListener::bind(&path.0).unwrap();
            assert!(listener.accept().now_or_never().is_none());
            let (client, server) = futures::join!(UnixStream::connect(&path.0), listener.accept());
            let mut client = client.unwrap();
            let (mut server, _) = server.unwrap();
            let mut buffer = [0; 1];
            assert!(server.read(&mut buffer).now_or_never().is_none());
            client.write_all(b"x").await.unwrap();
            server.read_exact(&mut buffer).await.unwrap();
            assert_eq!(buffer, *b"x");
            drop(client);
            assert_eq!(server.read(&mut buffer).await.unwrap(), 0);
        }));
    }

    #[test]
    fn missing_path_and_duplicate_bind_return_errors() {
        async_io::block_on(bounded(async {
            let path = SocketPath::new();
            assert!(UnixStream::connect(&path.0).await.is_err());
            let _listener = UnixListener::bind(&path.0).unwrap();
            assert!(UnixListener::bind(&path.0).is_err());
        }));
    }
}
