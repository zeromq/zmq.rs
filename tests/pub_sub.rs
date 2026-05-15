#[cfg(test)]
mod test {
    use bytes::Bytes;
    use zeromq::__async_rt as async_rt;
    use zeromq::prelude::*;
    use zeromq::Endpoint;
    use zeromq::ZmqMessage;

    use futures::channel::{mpsc, oneshot};
    use futures::{SinkExt, StreamExt};
    use std::convert::TryFrom;
    use std::time::Duration;

    async fn recv_text(sub_socket: &mut zeromq::SubSocket) -> String {
        let message = async_rt::task::timeout(Duration::from_secs(2), sub_socket.recv())
            .await
            .expect("timeout waiting for subscriber message")
            .expect("failed to receive subscriber message");
        String::from_utf8(message.get(0).unwrap().to_vec()).unwrap()
    }

    async fn wait_for_delivery(
        pub_socket: &mut zeromq::PubSocket,
        sub_socket: &mut zeromq::SubSocket,
        payload: &str,
    ) {
        for _ in 0..200 {
            pub_socket
                .send(ZmqMessage::from(payload))
                .await
                .expect("failed to send sync payload");
            match async_rt::task::timeout(Duration::from_millis(10), sub_socket.recv()).await {
                Ok(Ok(message)) if message.get(0).unwrap().as_ref() == payload.as_bytes() => {
                    return;
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => panic!("failed to receive sync payload: {e:?}"),
                Err(_) => async_rt::task::sleep(Duration::from_millis(5)).await,
            }
        }
        panic!("timed out waiting for subscription to receive {payload}");
    }

    async fn drain_pending(sub_socket: &mut zeromq::SubSocket) {
        loop {
            match async_rt::task::timeout(Duration::from_millis(10), sub_socket.recv()).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => panic!("failed to drain subscriber payload: {e:?}"),
                Err(_) => break,
            }
        }
    }

    async fn wait_for_delivery_to_both(
        pub_socket: &mut zeromq::PubSocket,
        left: &mut zeromq::SubSocket,
        right: &mut zeromq::SubSocket,
        payload: &str,
    ) {
        let mut left_ready = false;
        let mut right_ready = false;
        for _ in 0..200 {
            pub_socket
                .send(ZmqMessage::from(payload))
                .await
                .expect("failed to send sync payload");

            if !left_ready {
                match async_rt::task::timeout(Duration::from_millis(10), left.recv()).await {
                    Ok(Ok(message)) if message.get(0).unwrap().as_ref() == payload.as_bytes() => {
                        left_ready = true;
                    }
                    Ok(Err(e)) => panic!("failed to receive left sync payload: {e:?}"),
                    Ok(Ok(_)) | Err(_) => {}
                }
            }

            if !right_ready {
                match async_rt::task::timeout(Duration::from_millis(10), right.recv()).await {
                    Ok(Ok(message)) if message.get(0).unwrap().as_ref() == payload.as_bytes() => {
                        right_ready = true;
                    }
                    Ok(Err(e)) => panic!("failed to receive right sync payload: {e:?}"),
                    Ok(Ok(_)) | Err(_) => {}
                }
            }

            if left_ready && right_ready {
                drain_pending(left).await;
                drain_pending(right).await;
                return;
            }

            async_rt::task::sleep(Duration::from_millis(5)).await;
        }

        panic!("timed out waiting for both subscriptions to receive {payload}");
    }

    #[async_rt::test]
    async fn test_pub_late_subscriber_filter_state_is_isolated() {
        pretty_env_logger::try_init().ok();

        let mut pub_socket = zeromq::PubSocket::new();
        let endpoint = pub_socket
            .bind("tcp://127.0.0.1:0")
            .await
            .expect("Failed to bind");

        let mut early_sub = zeromq::SubSocket::new();
        early_sub
            .connect(&endpoint.to_string())
            .await
            .expect("Failed to connect early subscriber");
        early_sub
            .subscribe("early")
            .await
            .expect("Failed to subscribe early subscriber");
        wait_for_delivery(&mut pub_socket, &mut early_sub, "early-sync").await;

        let mut late_sub = zeromq::SubSocket::new();
        late_sub
            .connect(&endpoint.to_string())
            .await
            .expect("Failed to connect late subscriber");
        late_sub
            .subscribe("late")
            .await
            .expect("Failed to subscribe late subscriber");
        wait_for_delivery(&mut pub_socket, &mut late_sub, "late-sync").await;

        pub_socket
            .send(ZmqMessage::from("early-after-late-join"))
            .await
            .expect("Failed to send early message");
        pub_socket
            .send(ZmqMessage::from("late-after-join"))
            .await
            .expect("Failed to send late message");

        assert_eq!(recv_text(&mut early_sub).await, "early-after-late-join");
        assert_eq!(recv_text(&mut late_sub).await, "late-after-join");
    }

    #[async_rt::test]
    async fn test_pub_unsubscribe_stops_delivery_after_later_subscription() {
        pretty_env_logger::try_init().ok();

        let mut pub_socket = zeromq::PubSocket::new();
        let endpoint = pub_socket
            .bind("tcp://127.0.0.1:0")
            .await
            .expect("Failed to bind");

        let mut sub_socket = zeromq::SubSocket::new();
        sub_socket
            .connect(&endpoint.to_string())
            .await
            .expect("Failed to connect subscriber");
        sub_socket
            .subscribe("gone")
            .await
            .expect("Failed to subscribe initial filter");
        wait_for_delivery(&mut pub_socket, &mut sub_socket, "gone-sync").await;

        sub_socket
            .unsubscribe("gone")
            .await
            .expect("Failed to unsubscribe initial filter");
        sub_socket
            .subscribe("stay")
            .await
            .expect("Failed to subscribe replacement filter");
        wait_for_delivery(&mut pub_socket, &mut sub_socket, "stay-sync").await;

        pub_socket
            .send(ZmqMessage::from("gone-after-unsubscribe"))
            .await
            .expect("Failed to send unsubscribed message");
        assert!(
            async_rt::task::timeout(Duration::from_millis(150), sub_socket.recv())
                .await
                .is_err(),
            "subscriber received a message for an unsubscribed prefix"
        );

        pub_socket
            .send(ZmqMessage::from("stay-after-unsubscribe"))
            .await
            .expect("Failed to send replacement message");
        assert_eq!(recv_text(&mut sub_socket).await, "stay-after-unsubscribe");
    }

    #[async_rt::test]
    async fn test_pub_fanout_burst_reaches_all_subscribers() {
        pretty_env_logger::try_init().ok();

        let mut pub_socket = zeromq::PubSocket::new();
        let endpoint = pub_socket
            .bind("tcp://127.0.0.1:0")
            .await
            .expect("Failed to bind");

        let mut left = zeromq::SubSocket::new();
        left.connect(&endpoint.to_string())
            .await
            .expect("Failed to connect left subscriber");
        left.subscribe("burst")
            .await
            .expect("Failed to subscribe left subscriber");

        let mut right = zeromq::SubSocket::new();
        right
            .connect(&endpoint.to_string())
            .await
            .expect("Failed to connect right subscriber");
        right
            .subscribe("burst")
            .await
            .expect("Failed to subscribe right subscriber");

        wait_for_delivery_to_both(&mut pub_socket, &mut left, &mut right, "burst-sync").await;

        for idx in 0..256 {
            pub_socket
                .send(ZmqMessage::from(format!("burst-{idx:03}")))
                .await
                .expect("Failed to send burst message");
        }

        for idx in 0..256 {
            let expected = format!("burst-{idx:03}");
            assert_eq!(recv_text(&mut left).await, expected);
            assert_eq!(recv_text(&mut right).await, expected);
        }
    }

    #[async_rt::test]
    async fn test_pub_fanout_delivers_multipart_message() {
        pretty_env_logger::try_init().ok();

        let mut pub_socket = zeromq::PubSocket::new();
        let endpoint = pub_socket
            .bind("tcp://127.0.0.1:0")
            .await
            .expect("Failed to bind");

        let mut sub_socket = zeromq::SubSocket::new();
        sub_socket
            .connect(&endpoint.to_string())
            .await
            .expect("Failed to connect subscriber");
        sub_socket
            .subscribe("topic")
            .await
            .expect("Failed to subscribe subscriber");
        wait_for_delivery(&mut pub_socket, &mut sub_socket, "topic-sync").await;

        let message = ZmqMessage::try_from(vec![
            Bytes::from_static(b"topic.multipart"),
            Bytes::from_static(b"payload"),
        ])
        .expect("Failed to build multipart message");
        pub_socket
            .send(message)
            .await
            .expect("Failed to send multipart message");

        let received = async_rt::task::timeout(Duration::from_secs(2), sub_socket.recv())
            .await
            .expect("timeout waiting for multipart PUB message")
            .expect("failed to receive multipart PUB message");
        assert_eq!(received.len(), 2);
        assert_eq!(received.get(0).unwrap().as_ref(), b"topic.multipart");
        assert_eq!(received.get(1).unwrap().as_ref(), b"payload");
    }

    #[async_rt::test]
    async fn test_pub_fanout_continues_after_subscriber_disconnect() {
        pretty_env_logger::try_init().ok();

        let mut pub_socket = zeromq::PubSocket::new();
        let endpoint = pub_socket
            .bind("tcp://127.0.0.1:0")
            .await
            .expect("Failed to bind");

        let mut dropped_sub = zeromq::SubSocket::new();
        dropped_sub
            .connect(&endpoint.to_string())
            .await
            .expect("Failed to connect dropped subscriber");
        dropped_sub
            .subscribe("gone")
            .await
            .expect("Failed to subscribe dropped subscriber");
        wait_for_delivery(&mut pub_socket, &mut dropped_sub, "gone-sync").await;

        let mut live_sub = zeromq::SubSocket::new();
        live_sub
            .connect(&endpoint.to_string())
            .await
            .expect("Failed to connect live subscriber");
        live_sub
            .subscribe("live")
            .await
            .expect("Failed to subscribe live subscriber");
        wait_for_delivery(&mut pub_socket, &mut live_sub, "live-sync").await;

        drop(dropped_sub);
        async_rt::task::sleep(Duration::from_millis(50)).await;

        pub_socket
            .send(ZmqMessage::from("gone-after-drop"))
            .await
            .expect("Failed to send to dropped subscriber");
        pub_socket
            .send(ZmqMessage::from("live-after-drop"))
            .await
            .expect("Failed to send to live subscriber");

        assert_eq!(recv_text(&mut live_sub).await, "live-after-drop");
    }

    #[async_rt::test]
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
            task_handles.push(async_rt::task::spawn(async move {
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

                    let s: String = cloned_payload.clone();
                    let m = ZmqMessage::from(s);
                    pub_socket.send(m).await.expect("Failed to send");
                    async_rt::task::sleep(Duration::from_millis(1)).await;
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
                task_handles.push(async_rt::task::spawn(async move {
                    let mut sub_socket = zeromq::SubSocket::new();
                    sub_socket
                        .connect(&cloned_bound_addr)
                        .await
                        .unwrap_or_else(|_| panic!("Failed to connect to {}", bind_addr));

                    sub_socket.subscribe("").await.expect("Failed to subscribe");

                    async_rt::task::sleep(std::time::Duration::from_millis(500)).await;

                    for _ in 0..10 {
                        let recv_message = sub_socket.recv().await.unwrap();
                        let recv_payload =
                            String::from_utf8(recv_message.get(0).unwrap().to_vec()).unwrap();
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
}
