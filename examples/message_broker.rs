use std::convert::TryInto;
use std::error::Error;
use std::time::Duration;
use zeromq::{SocketFrontend, ZmqError};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut frontend = zeromq::RouterSocket::new();
    frontend
        .bind("127.0.0.1:5559")
        .await
        .expect("Failed to bind");

    // let mut backend = zmq_rs::DealerSocket::bind("127.0.0.1:5560")
    //     .await
    //     .expect("Failed to bind");
    //
    // zmq_rs::proxy(Box::new(frontend), Box::new(backend)).await?;
    loop {
        let mut mess = frontend.recv_multipart().await;
        match mess {
            Ok(mut message) => {
                dbg!(&message);
                let request: String = message.remove(2).try_into()?;

                message.push(format!("{} Reply", request).into());
                frontend.send_multipart(message).await?;
            }
            Err(ZmqError::NoMessage) => {
                println!("No messages");
            }
            Err(e) => {
                dbg!(e);
            }
        }
        tokio::time::delay_for(Duration::from_millis(500)).await;
    }
    drop(frontend);
    tokio::time::delay_for(Duration::from_millis(1000)).await;
    Ok(())
}
