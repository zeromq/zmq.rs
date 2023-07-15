use zeromq::prelude::*;
use zeromq::Endpoint;
use zeromq::ZmqMessage;
use zeromq::__async_rt as async_rt;

use futures_channel::oneshot;
use std::time::Duration;

#[async_rt::test]
async fn test_pair_sockets() {
    pretty_env_logger::try_init().ok();

    async fn helper(bind_addr: &'static str) {
        // We will join on these at the end to determine if any tasks we spawned
        // panicked
        let mut task_handles = Vec::new();
        let payload = chrono::Utc::now().to_rfc2822();

        let cloned_payload = payload.clone();
        let (has_bound_sender, has_bound) = oneshot::channel::<Endpoint>();
        task_handles.push(async_rt::task::spawn(async move {
            let mut pair_socket = zeromq::PairSocket::new();
            let bound_to = pair_socket
                .bind(bind_addr)
                .await
                .unwrap_or_else(|e| panic!("Failed to bind to {}: {}", bind_addr, e));
            has_bound_sender
                .send(bound_to)
                .expect("channel was dropped");

            let s: String = cloned_payload.clone();
            let m = ZmqMessage::from(s);
            pair_socket.send(m).await.expect("Failed to send");
            async_rt::task::sleep(Duration::from_millis(1)).await;

            let errs = pair_socket.close().await;
            if !errs.is_empty() {
                panic!("Could not unbind socket: {:?}", errs);
            }
        }));

        // Block until the pair socket has finished binding
        let bound_addr = has_bound.await.expect("channel was cancelled");
        if let Endpoint::Tcp(_host, port) = bound_addr.clone() {
            assert_ne!(port, 0);
        }

        let cloned_payload = payload.clone();
        let cloned_bound_addr = bound_addr.to_string();
        task_handles.push(async_rt::task::spawn(async move {
            let mut pair_socket = zeromq::PairSocket::new();
            pair_socket
                .connect(&cloned_bound_addr)
                .await
                .unwrap_or_else(|_| panic!("Failed to connect to {}", bind_addr));

            async_rt::task::sleep(std::time::Duration::from_millis(500)).await;

            let recv_message = pair_socket.recv().await.unwrap();
            let recv_payload = String::from_utf8(recv_message.get(0).unwrap().to_vec()).unwrap();
            assert_eq!(cloned_payload, recv_payload);
        }));

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
        "ipc://asdf-pair.sock",
        "ipc://anothersocket-pair-asdf",
    ];
    futures_util::future::join_all(addrs.into_iter().map(helper)).await;
}
