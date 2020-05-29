use bytes::Bytes;
use std::convert::TryInto;
use std::error::Error;
use zeromq::{Socket, SocketType};
use zeromq::{SubSocket, ZmqMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut socket = zeromq::SubSocket::connect("127.0.0.1:5556")
        .await
        .expect("Failed to connect");

    socket.subscribe("").await?;

    for i in 0..10 {
        println!("Message {}", i);
        let repl: String = socket.recv().await?.try_into()?;
        dbg!(repl);
    }
    Ok(())
}
