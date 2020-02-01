use std::error::Error;
use zmq::{Socket, SocketType, Message};
use bytes::Bytes;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut socket = Socket::connect("127.0.0.1:5555")
        .await
        .expect("Failed to connect");

    let hello = b"\x01\0\0\x05Hello";
    dbg!(hello);
    socket.send(Message::Bytes(Bytes::from_static(hello)), 0).await?;

    let data = socket.recv(0).await?;
    dbg!(data);
    Ok(())
}
