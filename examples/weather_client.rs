use bytes::Bytes;
use std::error::Error;
use zmq_rs::{ZmqMessage, SubSocket};
use zmq_rs::{Socket, SocketType};


#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut socket = zmq_rs::SubSocket::connect("127.0.0.1:5556")
        .await
        .expect("Failed to connect");

    socket.subscribe("").await?;

    for i in 0..10 {
        println!("Message {}", i);
        let data = socket.recv().await?;
        let repl = String::from_utf8(data)?;
        dbg!(repl);
    }
    Ok(())
}
