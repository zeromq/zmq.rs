use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use std::convert::TryInto;
use std::time::Duration;
use zeromq::prelude::*;

#[tokio::test]
async fn test_pub_sub_sockets() {
    let (server_stop_sender, mut server_stop) = oneshot::channel::<()>();
    let (results_sender, results) = mpsc::channel(100);
    tokio::spawn(async move {
        let mut pub_socket = zeromq::PubSocket::new();
        pub_socket
            .bind("127.0.0.1:5556")
            .await
            .expect("Failed to bind socket");

        loop {
            if let Ok(Some(_)) = server_stop.try_recv() {
                break;
            }
            pub_socket
                .send(chrono::Utc::now().to_rfc2822().into())
                .expect("Failed to send");
            tokio::time::delay_for(Duration::from_millis(100)).await;
        }
    });

    for _ in 0..10 {
        let mut client_sender = results_sender.clone();
        tokio::spawn(async move {
            let mut sub_socket = zeromq::SubSocket::new();
            sub_socket
                .connect("127.0.0.1:5556")
                .await
                .expect("Failed to connect");

            sub_socket.subscribe("").await.expect("Failed to subscribe");

            for _ in 0..10i32 {
                let repl: String = sub_socket
                    .recv()
                    .await
                    .expect("Failed to recv")
                    .try_into()
                    .expect("Malformed string");
                assert_eq!(chrono::Utc::now().to_rfc2822(), repl);
                client_sender.send(true).await.unwrap();
            }
        });
    }
    drop(results_sender);

    let res_vec: Vec<bool> = results.collect().await;
    server_stop_sender.send(()).unwrap();
    assert_eq!(100, res_vec.len());
}
