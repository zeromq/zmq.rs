use crate::{Socket, SocketType};
use std::convert::TryInto;
use std::io::Write;

#[tokio::test]
async fn test_pub_sub_sockets() {
    tokio::spawn(async move {
        let mut pub_socket = crate::PubSocket::bind("127.0.0.1:5556")
            .await
            .expect("Failed to bind socket");

        for i in 0..10i32 {
            let message = format!("Message - {}", i);
            pub_socket
                .send(message.into_bytes())
                .await
                .expect("Failed to send");
        }
    });

    tokio::spawn(async move {
        let mut sub_socket = crate::SubSocket::connect("127.0.0.1:5556")
            .await
            .expect("Failed to connect");

        sub_socket.subscribe("").await.expect("Failed to subscribe");

        for i in 0..10i32 {
            let data = sub_socket.recv().await.expect("Failed to recv");
            let repl = String::from_utf8(data).expect("Malformed string");
            assert_eq!(format!("Message - {}", i), repl)
        }
    });
}
