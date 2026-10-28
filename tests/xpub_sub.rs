#[cfg(test)]
mod test {
    use zeromq::__async_rt as async_rt;
    use zeromq::prelude::*;
    use zeromq::ZmqMessage;

    use std::time::Duration;

    async fn recv_text(sub_socket: &mut zeromq::SubSocket) -> String {
        let message = async_rt::task::timeout(Duration::from_secs(2), sub_socket.recv())
            .await
            .expect("timeout waiting for subscriber message")
            .expect("failed to receive subscriber message");
        String::from_utf8(message.get(0).unwrap().to_vec()).unwrap()
    }

    #[async_rt::test]
    async fn test_xpub_basic_pubsub() {
        pretty_env_logger::try_init().ok();

        let mut xpub_socket = zeromq::XPubSocket::new();
        let bound_to = xpub_socket
            .bind("tcp://127.0.0.1:0")
            .await
            .expect("Failed to bind");

        let bound_addr = bound_to.to_string();

        // Spawn SUB socket
        let sub_handle = async_rt::task::spawn(async move {
            let mut sub_socket = zeromq::SubSocket::new();
            sub_socket
                .connect(&bound_addr)
                .await
                .expect("Failed to connect");

            sub_socket.subscribe("").await.expect("Failed to subscribe");

            // Wait a bit for subscription to propagate
            async_rt::task::sleep(Duration::from_millis(200)).await;

            // Receive 5 messages
            let mut received = Vec::new();
            for _ in 0..5 {
                let msg = sub_socket.recv().await.expect("Failed to receive");
                let data = String::from_utf8(msg.get(0).unwrap().to_vec()).unwrap();
                received.push(data);
            }
            received
        });

        // XPUB receives subscription message
        let sub_msg = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
            .await
            .expect("Timeout waiting for subscription")
            .expect("Failed to receive subscription");

        let data = sub_msg.get(0).unwrap();
        assert_eq!(data[0], 1); // Subscribe byte
        assert_eq!(&data[1..], b""); // Empty subscription (subscribe to all)

        // Give time for subscription to be fully processed
        async_rt::task::sleep(Duration::from_millis(100)).await;

        // Send messages
        for i in 0..5 {
            let msg = ZmqMessage::from(format!("message-{}", i));
            xpub_socket.send(msg).await.expect("Failed to send");
        }

        // Wait for SUB to receive all messages
        let received = sub_handle.await.expect("SUB task failed");
        assert_eq!(received.len(), 5);
        for (i, msg) in received.iter().enumerate() {
            assert_eq!(msg, &format!("message-{}", i));
        }
    }

    #[async_rt::test]
    async fn test_xpub_receives_unsubscribe() {
        pretty_env_logger::try_init().ok();

        let mut xpub_socket = zeromq::XPubSocket::new();
        let bound_to = xpub_socket
            .bind("tcp://127.0.0.1:0")
            .await
            .expect("Failed to bind");

        let bound_addr = bound_to.to_string();
        let handle = async_rt::task::spawn(async move {
            let mut sub_socket = zeromq::SubSocket::new();
            sub_socket
                .connect(&bound_addr)
                .await
                .expect("Failed to connect");

            // Subscribe
            sub_socket
                .subscribe("test")
                .await
                .expect("Failed to subscribe");
            async_rt::task::sleep(Duration::from_millis(100)).await;

            // Unsubscribe
            sub_socket
                .unsubscribe("test")
                .await
                .expect("Failed to unsubscribe");
            async_rt::task::sleep(Duration::from_millis(100)).await;
        });

        // Receive subscribe message
        let sub_msg = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
            .await
            .expect("Timeout")
            .expect("Failed to receive");

        let data = sub_msg.get(0).unwrap();
        assert_eq!(data[0], 1); // Subscribe byte
        assert_eq!(&data[1..], b"test");

        // Receive unsubscribe message
        let unsub_msg = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
            .await
            .expect("Timeout")
            .expect("Failed to receive");

        let data = unsub_msg.get(0).unwrap();
        assert_eq!(data[0], 0); // Unsubscribe byte
        assert_eq!(&data[1..], b"test");

        handle.await.expect("Task failed");
    }

    #[async_rt::test]
    async fn test_xpub_filtered_subscriptions() {
        pretty_env_logger::try_init().ok();

        let mut xpub_socket = zeromq::XPubSocket::new();
        let bound_to = xpub_socket
            .bind("tcp://127.0.0.1:0")
            .await
            .expect("Failed to bind");

        let bound_addr = bound_to.to_string();

        // Spawn SUB socket that subscribes to "topic1"
        let sub_handle = async_rt::task::spawn(async move {
            let mut sub_socket = zeromq::SubSocket::new();
            sub_socket
                .connect(&bound_addr)
                .await
                .expect("Failed to connect");

            sub_socket
                .subscribe("topic1")
                .await
                .expect("Failed to subscribe");

            async_rt::task::sleep(Duration::from_millis(200)).await;

            // Should only receive messages starting with "topic1"
            let msg = sub_socket.recv().await.expect("Failed to receive");
            String::from_utf8(msg.get(0).unwrap().to_vec()).unwrap()
        });

        // Receive subscription
        let _sub_msg = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
            .await
            .expect("Timeout")
            .expect("Failed to receive subscription");

        async_rt::task::sleep(Duration::from_millis(100)).await;

        // Send messages with different topics
        xpub_socket
            .send(ZmqMessage::from("topic2-message"))
            .await
            .expect("Failed to send");
        xpub_socket
            .send(ZmqMessage::from("topic1-message"))
            .await
            .expect("Failed to send");
        xpub_socket
            .send(ZmqMessage::from("topic3-message"))
            .await
            .expect("Failed to send");

        // SUB should only receive "topic1-message"
        let received = sub_handle.await.expect("SUB task failed");
        assert_eq!(received, "topic1-message");
    }

    #[async_rt::test]
    async fn test_xpub_late_subscriber_filter_state_is_isolated() {
        pretty_env_logger::try_init().ok();

        let mut xpub_socket = zeromq::XPubSocket::new();
        let endpoint = xpub_socket
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

        let early_sub_msg = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
            .await
            .expect("Timeout waiting for early subscription")
            .expect("Failed to receive early subscription");
        assert_eq!(early_sub_msg.get(0).unwrap().as_ref(), b"\x01early");

        let mut late_sub = zeromq::SubSocket::new();
        late_sub
            .connect(&endpoint.to_string())
            .await
            .expect("Failed to connect late subscriber");
        late_sub
            .subscribe("late")
            .await
            .expect("Failed to subscribe late subscriber");

        let late_sub_msg = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
            .await
            .expect("Timeout waiting for late subscription")
            .expect("Failed to receive late subscription");
        assert_eq!(late_sub_msg.get(0).unwrap().as_ref(), b"\x01late");

        xpub_socket
            .send(ZmqMessage::from("early-after-late-join"))
            .await
            .expect("Failed to send early message");
        xpub_socket
            .send(ZmqMessage::from("late-after-join"))
            .await
            .expect("Failed to send late message");

        assert_eq!(recv_text(&mut early_sub).await, "early-after-late-join");
        assert_eq!(recv_text(&mut late_sub).await, "late-after-join");
    }

    #[async_rt::test]
    async fn test_xpub_unsubscribe_updates_fanout_state() {
        pretty_env_logger::try_init().ok();

        let mut xpub_socket = zeromq::XPubSocket::new();
        let endpoint = xpub_socket
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

        let sub_msg = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
            .await
            .expect("Timeout waiting for subscription")
            .expect("Failed to receive subscription");
        assert_eq!(sub_msg.get(0).unwrap().as_ref(), b"\x01gone");

        xpub_socket
            .send(ZmqMessage::from("gone-before-unsubscribe"))
            .await
            .expect("Failed to send subscribed message");
        assert_eq!(recv_text(&mut sub_socket).await, "gone-before-unsubscribe");

        sub_socket
            .unsubscribe("gone")
            .await
            .expect("Failed to unsubscribe initial filter");
        let unsub_msg = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
            .await
            .expect("Timeout waiting for unsubscribe")
            .expect("Failed to receive unsubscribe");
        assert_eq!(unsub_msg.get(0).unwrap().as_ref(), b"\x00gone");

        sub_socket
            .subscribe("stay")
            .await
            .expect("Failed to subscribe replacement filter");
        let replacement_sub = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
            .await
            .expect("Timeout waiting for replacement subscription")
            .expect("Failed to receive replacement subscription");
        assert_eq!(replacement_sub.get(0).unwrap().as_ref(), b"\x01stay");

        xpub_socket
            .send(ZmqMessage::from("gone-after-unsubscribe"))
            .await
            .expect("Failed to send unsubscribed message");
        assert!(
            async_rt::task::timeout(Duration::from_millis(150), sub_socket.recv())
                .await
                .is_err(),
            "subscriber received a message for an unsubscribed prefix"
        );

        xpub_socket
            .send(ZmqMessage::from("stay-after-unsubscribe"))
            .await
            .expect("Failed to send replacement message");
        assert_eq!(recv_text(&mut sub_socket).await, "stay-after-unsubscribe");
    }
}
