use bytes::Bytes;
use std::convert::TryInto;
use std::error::Error;
use zeromq::{BlockingRecv, BlockingSend, Socket, SocketType};
use zeromq::{SocketFrontend, ZmqMessage};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut socket = zeromq::ReqSocket::new();
    socket
        .connect("127.0.0.1:5555")
        .await
        .expect("Failed to connect");
    println!("Connected to server");

    socket.send("Hello".into()).await?;
    let repl: String = socket.recv().await?.try_into()?;
    dbg!(repl);

    socket.send("NewHello".into()).await?;
    let repl: String = socket.recv().await?.try_into()?;
    dbg!(repl);
    Ok(())
}
