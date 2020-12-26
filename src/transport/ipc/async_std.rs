use crate::codec::FramedIo;
use crate::endpoint::Endpoint;
use crate::ZmqResult;

use std::path::PathBuf;
use std::sync::Arc;

pub(crate) async fn connect(path: PathBuf) -> ZmqResult<(FramedIo, Endpoint)> {
    let raw_socket = async_std::os::unix::net::UnixStream::connect(&path).await?;
    let peer_addr = raw_socket.peer_addr()?;
    let peer_addr = peer_addr.as_pathname().map(|a| a.to_owned());
    // Async-std doesn't have tokio's split function, but this works just as well
    let read = Arc::new(raw_socket);
    let write = read.clone();
    let raw_sock = FramedIo::new(Box::new(read), Box::new(write));
    Ok((raw_sock, Endpoint::Ipc(peer_addr)))
}
