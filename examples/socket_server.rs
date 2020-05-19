use tokio::net::TcpListener;
use tokio::prelude::*;

use zeromq::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Start server");
    let mut server = zeromq::bind(SocketType::REP, "127.0.0.1:5555").await?;

    loop {
        let mut socket = server.accept().await?;

        tokio::spawn(async move {
            loop {
                let message = socket.recv().await.expect("Failed to receive");
                let mut repl = String::from_utf8(message).unwrap();
                dbg!(&repl);
                repl.push_str(" Reply");
                socket
                    .send(repl.into_bytes())
                    .await
                    .expect("Failed to send");
            }
        });
    }
}
