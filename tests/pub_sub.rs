use zeromq::prelude::*;
use zeromq::Endpoint;

use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use std::convert::TryInto;
use std::time::Duration;

#[tokio::test]
async fn test_pub_sub_sockets() {
    pretty_env_logger::try_init().ok();

    async fn helper(bind_addr: &'static str) {
        // We will join on these at the end to determine if any tasks we spawned
        // panicked
        let mut task_handles = Vec::new();
        let payload = chrono::Utc::now().to_rfc2822();

        let cloned_payload = payload.clone();
        let (server_stop_sender, mut server_stop) = oneshot::channel::<()>();
        let (has_bound_sender, has_bound) = oneshot::channel::<Endpoint>();
        task_handles.push(tokio::spawn(async move {
            let mut pub_socket = zeromq::PubSocket::new();
            let bound_to = pub_socket
                .bind(bind_addr)
                .await
                .unwrap_or_else(|e| panic!("Failed to bind to {}: {}", bind_addr, e));
            has_bound_sender
                .send(bound_to)
                .expect("channel was dropped");

            loop {
                if let Ok(Some(_)) = server_stop.try_recv() {
                    break;
                }

                pub_socket
                    .send(cloned_payload.clone().into())
                    .await
                    .expect("Failed to send");
                tokio::time::delay_for(Duration::from_millis(1)).await;
            }

            let errs = pub_socket.close().await;
            if !errs.is_empty() {
                panic!("Could not unbind socket: {:?}", errs);
            }
        }));
        // Block until the pub has finished binding
        // TODO: ZMQ sockets should not care about this sort of ordering.
        // See https://github.com/zeromq/zmq.rs/issues/73
        let bound_addr = has_bound.await.expect("channel was cancelled");
        if let Endpoint::Tcp(_host, port) = bound_addr.clone() {
            assert_ne!(port, 0);
        }

        let (sub_results_sender, sub_results) = mpsc::channel(100);
        for _ in 0..10 {
            let mut cloned_sub_sender = sub_results_sender.clone();
            let cloned_payload = payload.clone();
            let cloned_bound_addr = bound_addr.to_string();
            task_handles.push(tokio::spawn(async move {
                let mut sub_socket = zeromq::SubSocket::new();
                sub_socket
                    .connect(&cloned_bound_addr)
                    .await
                    .unwrap_or_else(|_| panic!("Failed to connect to {}", bind_addr));

                sub_socket.subscribe("").await.expect("Failed to subscribe");

                tokio::time::delay_for(std::time::Duration::from_millis(500)).await;

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
            }));
        }
        drop(sub_results_sender);
        let res_vec: Vec<()> = sub_results.collect().await;
        assert_eq!(100, res_vec.len());

        server_stop_sender.send(()).unwrap();
        for t in task_handles {
            t.await.expect("Task failed unexpectedly!");
        }
    }

    let addrs = vec![
        "tcp://localhost:0",
        "tcp://127.0.0.1:0",
        "tcp://[::1]:0",
        "tcp://127.0.0.1:0",
        "tcp://localhost:0",
        "tcp://127.0.0.1:0",
        "tcp://[::1]:0",
        "ipc://asdf.sock",
        "ipc://anothersocket-asdf",
    ];
    futures::future::join_all(addrs.into_iter().map(helper)).await;
}
