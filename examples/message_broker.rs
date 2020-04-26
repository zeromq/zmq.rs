use std::error::Error;
use zmq_rs::ZmqMessage;
use zmq_rs::{Socket, SocketType};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut frontend = zmq_rs::RouterSocket::bind("127.0.0.1:5559")
        .await
        .expect("Failed to bind");

    let mut backend = zmq_rs::DealerSocket::bind("127.0.0.1:5560")
        .await
        .expect("Failed to bind");

    zmq_rs::proxy(Box::new(frontend), Box::new(backend)).await?;

    Ok(())
}
