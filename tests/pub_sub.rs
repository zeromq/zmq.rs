use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use std::convert::TryInto;
use std::time::Duration;
use zeromq::prelude::*;

#[tokio::test]
async fn test_pub_sub_sockets() {
    async fn helper(bind_addr: &'static str) {
        let payload = chrono::Utc::now().to_rfc2822();

        let cloned_payload = payload.clone();
        let (server_stop_sender, mut server_stop) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let mut pub_socket = zeromq::PubSocket::new();
            pub_socket
                .bind(bind_addr)
                .await
                .unwrap_or_else(|_| panic!("Failed to bind to {}", bind_addr));

            loop {
                if let Ok(Some(_)) = server_stop.try_recv() {
                    break;
                }

                pub_socket
                    .send(cloned_payload.clone().into())
                    .expect("Failed to send");
                tokio::time::delay_for(Duration::from_millis(1)).await;
            }
        });

        let (sub_results_sender, sub_results) = mpsc::channel(100);
        for _ in 0..10 {
            let mut cloned_sub_sender = sub_results_sender.clone();
            let cloned_payload = payload.clone();
            tokio::spawn(async move {
                let mut sub_socket = zeromq::SubSocket::new();
                sub_socket
                    .connect(bind_addr)
                    .await
                    .unwrap_or_else(|_| panic!("Failed to connect to {}", bind_addr));

                sub_socket.subscribe("").await.expect("Failed to subscribe");

                for _ in 0..10 {
                    let recv_payload: String = sub_socket
                        .recv()
                        .await
                        .expect("Failed to recv")
                        .try_into()
                        .expect("Malformed string");
                    assert_eq!(cloned_payload, recv_payload);
                    cloned_sub_sender.send(()).await.unwrap();
                }
            });
        }
        drop(sub_results_sender);
        let res_vec: Vec<()> = sub_results.collect().await;
        assert_eq!(100, res_vec.len());

        server_stop_sender.send(()).unwrap();
    }

    let addrs = vec![
        "tcp://localhost:5553",
        "tcp://127.0.0.1:5554",
        "tcp://[::1]:5555",
        "tcp://127.0.0.1:5556",
    ];
    futures::future::join_all(addrs.into_iter().map(helper)).await;
}
