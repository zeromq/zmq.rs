mod async_helpers;

use std::convert::TryFrom;
use std::error::Error;
use zeromq::util::{PeerIdentity, TcpKeepalive};
use zeromq::{Socket, SocketOptions, SocketRecv, SocketSend};

#[async_helpers::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut options = SocketOptions::default();
    options
        .peer_identity(PeerIdentity::try_from(Vec::from("SomeCustomId")).unwrap())
        .tcp_keepalive(TcpKeepalive {
            keepalive: 1,
            count: 5,
            idle: 1,
            interval: 10,
        });

    let mut socket = zeromq::ReqSocket::with_options(options);
    socket
        .connect("tcp://127.0.0.1:5555")
        .await
        .expect("Failed to connect");
    println!("Connected to server");

    for _ in 0..10u64 {
        socket.send("Hello".into()).await?;
        let repl = socket.recv().await?;
        dbg!(repl);
    }
    Ok(())
}
