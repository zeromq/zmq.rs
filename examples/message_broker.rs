mod async_helpers;

use std::error::Error;
use zeromq::prelude::*;

use futures::{select, FutureExt};

#[async_helpers::main]
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
        select! {
            router_mess = frontend.recv_multipart().fuse() => {
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
            dealer_mess = backend.recv_multipart().fuse() => {
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
