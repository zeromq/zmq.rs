use bytes::Bytes;
use std::error::Error;
use zmq_rs::{Socket, SocketType};
use zmq_rs::ZmqMessage;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut socket = zmq_rs::connect(SocketType::REQ, "127.0.0.1:5555")
        .await
        .expect("Failed to connect");

    let hello = b"\x01\0\0\x05Hello";
    dbg!(hello);
    socket
        .send(ZmqMessage { data: Bytes::from_static(hello), more: false })
        .await?;

    let data = socket.recv().await?;
    dbg!(data);
    Ok(())
}
