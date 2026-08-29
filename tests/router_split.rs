#[cfg(test)]
mod test {

    use bytes::Bytes;
    use futures::future::{select, Either};
    use futures::pin_mut;
    use futures::StreamExt;
    use zeromq::__async_rt as async_rt;
    use zeromq::prelude::*;
    use zeromq::{SocketEvent, SocketOptions, ZmqMessage};

    use std::error::Error;
    use std::time::Duration;

    fn assert_send<T: Send>() {}
    fn assert_clone<T: Clone>() {}

    #[test]
    fn split_halves_are_send() {
        assert_send::<zeromq::RouterSendHalf>();
        assert_send::<zeromq::RouterRecvHalf>();
    }

    #[test]
    fn router_send_half_is_clone() {
        assert_clone::<zeromq::RouterSendHalf>();
    }

    #[async_rt::test]
    async fn router_monitor_reports_peer_disconnect_after_split() -> Result<(), Box<dyn Error>> {
        let peer_id: zeromq::util::PeerIdentity = "disconnecting-dealer".parse()?;
        let mut options = SocketOptions::default();
        options.peer_identity(peer_id.clone());

        let mut router = zeromq::RouterSocket::new();
        let mut monitor = router.monitor();
        let endpoint = router.bind("tcp://localhost:0").await?;

        let mut dealer = zeromq::DealerSocket::with_options(options);
        dealer.connect(endpoint.to_string().as_str()).await?;
        dealer.send(ZmqMessage::from("register-dealer")).await?;

        let (_send_half, mut recv_half) = router.split();
        let registration = recv_half.recv().await?;
        assert_eq!(registration.get(0).unwrap().as_ref(), peer_id.as_ref());

        assert!(dealer.close().await.is_empty());

        let disconnected_peer = async_rt::task::timeout(Duration::from_secs(5), async {
            loop {
                let recv = recv_half.recv();
                let next_event = monitor.next();
                pin_mut!(recv, next_event);

                match select(recv, next_event).await {
                    Either::Left((result, _)) => {
                        panic!("router recv completed before disconnect event: {result:?}")
                    }
                    Either::Right((Some(SocketEvent::Disconnected(disconnected_peer)), _)) => {
                        break disconnected_peer;
                    }
                    Either::Right((Some(_), _)) => {}
                    Either::Right((None, _)) => {
                        panic!("router monitor closed before disconnect event")
                    }
                }
            }
        })
        .await
        .expect("timeout waiting for disconnect monitor event");

        assert_eq!(disconnected_peer, peer_id);
        Ok(())
    }

    #[async_rt::test]
    async fn test_router_split_concurrent_send_recv() -> Result<(), Box<dyn Error>> {
        pretty_env_logger::try_init().ok();

        // ROUTER binds, DEALER connects
        let mut router = zeromq::RouterSocket::new();
        let endpoint = router.bind("tcp://localhost:0").await?;

        let mut dealer = zeromq::DealerSocket::new();
        dealer.connect(endpoint.to_string().as_str()).await?;

        async_rt::task::sleep(Duration::from_millis(100)).await;

        // Split the router into send/recv halves
        let (mut send_half, mut recv_half) = router.split();

        let num_messages: u32 = 5;

        // Dealer sends messages
        let dealer_task = async_rt::task::spawn(async move {
            for i in 0..num_messages {
                dealer
                    .send(ZmqMessage::from(format!("msg-{}", i)))
                    .await
                    .unwrap();
            }
            // Wait for all replies
            for _ in 0..num_messages {
                let _reply = dealer.recv().await.unwrap();
            }
        });

        // Router recv half receives identity-framed messages
        // Router send half echoes them back using the identity
        let echo_task = async_rt::task::spawn(async move {
            for _ in 0..num_messages {
                // recv gives [identity, ...payload]
                let msg = recv_half.recv().await.unwrap();
                // send expects [identity, ...payload] to route back
                send_half.send(msg).await.unwrap();
            }
        });

        let timeout = Duration::from_secs(5);
        async_rt::task::timeout(timeout, async {
            echo_task.await.unwrap();
            dealer_task.await.unwrap();
        })
        .await
        .expect("test timed out");

        Ok(())
    }

    #[async_rt::test]
    async fn test_router_split_send_half_clone_concurrent_send() -> Result<(), Box<dyn Error>> {
        pretty_env_logger::try_init().ok();

        fn message_for(identity: Bytes, payload: String) -> ZmqMessage {
            let mut msg = ZmqMessage::from(payload);
            msg.push_front(identity);
            msg
        }

        // ROUTER binds, DEALER connects
        let mut router = zeromq::RouterSocket::new();
        let endpoint = router.bind("tcp://localhost:0").await?;

        let mut dealer = zeromq::DealerSocket::new();
        dealer.connect(endpoint.to_string().as_str()).await?;

        async_rt::task::sleep(Duration::from_millis(100)).await;

        // Split the router into send/recv halves and clone the send halves
        let (send_half, mut recv_half) = router.split();
        let mut send_half_1 = send_half.clone();
        let mut send_half_2 = send_half;

        let num_messages: u32 = 10;

        // Dealer sends one message so router recv half can learn its identity
        dealer.send(ZmqMessage::from("register-dealer")).await?;
        let registration = recv_half.recv().await?;
        let dealer_identity = registration.get(0).unwrap().clone();

        let mut expected = Vec::new();
        for i in 0..num_messages {
            expected.push(format!("sender-1->{i}"));
            expected.push(format!("sender-2->{i}"));
        }
        expected.sort();

        // First cloned send half sends its own series of messages
        let dealer_identity_1 = dealer_identity.clone();
        let send_task_1 = async_rt::task::spawn(async move {
            for i in 0..num_messages {
                let message = message_for(dealer_identity_1.clone(), format!("sender-1->{i}"));
                send_half_1.send(message).await.unwrap();
            }
        });

        // Second cloned send half sends concurrently to the same dealer
        let dealer_identity_2 = dealer_identity.clone();
        let send_task_2 = async_rt::task::spawn(async move {
            for i in 0..num_messages {
                let message = message_for(dealer_identity_2.clone(), format!("sender-2->{i}"));
                send_half_2.send(message).await.unwrap();
            }
        });

        // Dealer receives the full set of messages from both senders
        let dealer_task = async_rt::task::spawn(async move {
            let mut received = Vec::new();
            for _ in 0..(num_messages * 2) {
                let reply = dealer.recv().await.unwrap();
                received.push(String::try_from(reply).unwrap());
            }
            received.sort();
            received
        });

        let timeout = Duration::from_secs(5);
        async_rt::task::timeout(timeout, async {
            send_task_1.await.unwrap();
            send_task_2.await.unwrap();
            assert_eq!(dealer_task.await.unwrap(), expected);
        })
        .await
        .expect("test timed out");

        Ok(())
    }
}
