use bytes::Bytes;
use std::error::Error;
use zmq_rs::ZmqMessage;
use zmq_rs::{Socket, SocketType};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut socket = zmq_rs::ReqSocket::connect("127.0.0.1:5555")
        .await
        .expect("Failed to connect");

    let hello = Vec::from("Hello");
    socket.send(hello).await?;
    let data = socket.recv().await?;
    let repl = String::from_utf8(data)?;
    dbg!(repl);

    let hello = Vec::from("NewHello");
    socket.send(hello).await?;
    let data = socket.recv().await?;
    let repl = String::from_utf8(data)?;
    dbg!(repl);
    Ok(())
}
