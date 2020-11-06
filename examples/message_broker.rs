use std::error::Error;
use zeromq::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut frontend = zeromq::RouterSocket::new();
    frontend
        .bind("tcp://127.0.0.1:5559")
        .await
        .expect("Failed to bind");

    let mut backend = zeromq::DealerSocket::new();
    backend
        .bind("tcp://127.0.0.1:5560")
        .await
        .expect("Failed to bind");
    loop {
        tokio::select! {
            router_mess = frontend.recv_multipart() => {
                dbg!(&router_mess);
                match router_mess {
                    Ok(message) => {
                        backend.send_multipart(message).await?;
                    }
                    Err(_) => {
                        todo!()
                    }
                }
            },
            dealer_mess = backend.recv_multipart() => {
                dbg!(&dealer_mess);
                match dealer_mess {
                    Ok(message) => {
                        frontend.send_multipart(message).await?;
                    }
                    Err(_) => {
                        todo!()
                    }
                }
            }
        };
    }
}
