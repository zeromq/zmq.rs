use crate::{Socket, SocketType};
use std::io::Write;

#[tokio::test]
async fn test_connect_socket() {
    let mut socket = Socket::connect("127.0.0.1:5555")
        .await
        .expect("Failed to connect");

    loop {
        let data = socket.recv(0).await;
        dbg!(data);
        std::io::stdout().flush();
    }
    assert!(false)
}
