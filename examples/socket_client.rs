use std::error::Error;
use zmq::{Socket, SocketType};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut socket = Socket::connect("127.0.0.1:5555")
        .await
        .expect("Failed to connect");

    let data = socket.recv(0).await?;
    Ok(())
}
