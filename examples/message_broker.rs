use std::error::Error;
use std::time::Duration;
use zmq_rs::ZmqMessage;
use zmq_rs::{Socket, SocketType};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut frontend = zmq_rs::RouterSocket::bind("127.0.0.1:5559")
        .await
        .expect("Failed to bind");

    // let mut backend = zmq_rs::DealerSocket::bind("127.0.0.1:5560")
    //     .await
    //     .expect("Failed to bind");
    //
    // zmq_rs::proxy(Box::new(frontend), Box::new(backend)).await?;
    loop {
        let mess = frontend.recv().await;
        dbg!(mess);
        tokio::time::delay_for(Duration::from_millis(500)).await;
    }
    drop(frontend);
    tokio::time::delay_for(Duration::from_millis(1000)).await;
    Ok(())
}
