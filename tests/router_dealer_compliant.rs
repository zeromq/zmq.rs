//! Conformance tests for ROUTER/DEALER sockets against libzmq.
//!
//! Tests identity-based routing between zmq.rs and libzmq implementations.

mod compliance;
use compliance::{get_monitor_event, join_thread, setup_monitor};

use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::ZmqMessage;

use std::convert::TryInto;
use std::time::Duration;

#[cfg(test)]
mod test {
    use super::*;

    // =========================================================================
    // Test 1: Our ROUTER binds, their DEALER connects
    // =========================================================================

    async fn setup_our_router(bind_endpoint: &str) -> (zeromq::RouterSocket, String) {
        let mut our_router = zeromq::RouterSocket::new();
        let endpoint = our_router
            .bind(bind_endpoint)
            .await
            .expect("Failed to bind");
        (our_router, endpoint.to_string())
    }

    fn setup_their_dealer(
        ctx: &zmq2::Context,
        connect_endpoint: &str,
        identity: &[u8],
    ) -> (zmq2::Socket, zmq2::Socket) {
        let their_dealer = ctx
            .socket(zmq2::DEALER)
            .expect("Couldn't make dealer socket");
        their_dealer
            .set_identity(identity)
            .expect("Failed to set identity");
        their_dealer
            .connect(connect_endpoint)
            .expect("Failed to connect");

        let their_monitor = setup_monitor(ctx, &their_dealer, "inproc://dealer-monitor");
        (their_dealer, their_monitor)
    }

    #[async_rt::test]
    async fn test_our_router_their_dealer() {
        pretty_env_logger::try_init().ok();

        let (mut our_router, bind_endpoint) = setup_our_router("tcp://127.0.0.1:0").await;
        println!("Our ROUTER bound to {}", bind_endpoint);

        let ctx = zmq2::Context::new();
        let identity = b"dealer-identity-1";
        let (their_dealer, _their_monitor) = setup_their_dealer(&ctx, &bind_endpoint, identity);

        // Allow connection and handshake to complete
        async_rt::task::sleep(Duration::from_millis(200)).await;

        const NUM_MSGS: u32 = 10;

        // Their DEALER sends messages in a thread
        let dealer_handle = std::thread::spawn(move || {
            for i in 0..NUM_MSGS {
                their_dealer
                    .send(&format!("Request: {}", i), 0)
                    .expect("Failed to send");

                let reply = their_dealer
                    .recv_string(0)
                    .expect("Failed to recv")
                    .expect("Invalid UTF8");
                assert_eq!(reply, format!("Reply: {}", i));
            }
            their_dealer
        });

        // Our ROUTER receives and replies
        for i in 0..NUM_MSGS {
            let msg = our_router.recv().await.expect("Failed to recv");

            // ROUTER recv prepends identity frame
            assert_eq!(msg.len(), 2, "Expected [identity, payload]");
            let recv_identity = msg.get(0).unwrap();
            assert_eq!(recv_identity.as_ref(), identity);

            let payload = String::from_utf8(msg.into_vec().pop().unwrap().to_vec()).unwrap();
            assert_eq!(payload, format!("Request: {}", i));

            // Send reply with identity frame
            let mut reply = ZmqMessage::from(format!("Reply: {}", i));
            reply.push_front(identity.to_vec().into());
            our_router.send(reply).await.expect("Failed to send");
        }

        join_thread(dealer_handle, Duration::from_secs(5), "dealer thread").await;
    }

    // =========================================================================
    // Test 2: Their ROUTER binds, our DEALER connects
    // =========================================================================

    fn setup_their_router(bind_endpoint: &str) -> (zmq2::Socket, String, zmq2::Socket) {
        let ctx = zmq2::Context::new();
        let their_router = ctx
            .socket(zmq2::ROUTER)
            .expect("Couldn't make router socket");
        their_router.bind(bind_endpoint).expect("Failed to bind");

        let resolved_bind = their_router.get_last_endpoint().unwrap().unwrap();
        let their_monitor = setup_monitor(&ctx, &their_router, "inproc://router-monitor");

        (their_router, resolved_bind, their_monitor)
    }

    async fn setup_our_dealer(connect_endpoint: &str) -> zeromq::DealerSocket {
        let mut our_dealer = zeromq::DealerSocket::new();
        our_dealer
            .connect(connect_endpoint)
            .await
            .expect("Failed to connect");
        our_dealer
    }

    #[async_rt::test]
    async fn test_their_router_our_dealer() {
        pretty_env_logger::try_init().ok();

        let (their_router, bind_endpoint, their_monitor) = setup_their_router("tcp://127.0.0.1:0");
        println!("Their ROUTER bound to {}", bind_endpoint);

        let mut our_dealer = setup_our_dealer(&bind_endpoint).await;

        // Wait for handshake
        assert_eq!(
            zmq2::SocketEvent::ACCEPTED,
            get_monitor_event(&their_monitor).0
        );
        assert_eq!(
            zmq2::SocketEvent::HANDSHAKE_SUCCEEDED,
            get_monitor_event(&their_monitor).0
        );

        async_rt::task::sleep(Duration::from_millis(100)).await;

        const NUM_MSGS: u32 = 10;

        // Their ROUTER runs in a thread (blocking recv)
        let router_handle = std::thread::spawn(move || {
            for i in 0..NUM_MSGS {
                // ROUTER recv returns [identity, ...payload]
                let parts = their_router.recv_multipart(0).expect("Failed to recv");
                assert_eq!(parts.len(), 2, "Expected [identity, payload]");

                let identity = &parts[0];
                let payload = String::from_utf8(parts[1].clone()).unwrap();
                assert_eq!(payload, format!("Request: {}", i));

                // Send reply with identity
                their_router
                    .send_multipart([identity.as_slice(), format!("Reply: {}", i).as_bytes()], 0)
                    .expect("Failed to send");
            }
            their_router
        });

        // Our DEALER sends and receives
        for i in 0..NUM_MSGS {
            let msg = ZmqMessage::from(format!("Request: {}", i));
            our_dealer.send(msg).await.expect("Failed to send");

            let reply = our_dealer.recv().await.expect("Failed to recv");
            let reply_str: String = reply.try_into().unwrap();
            assert_eq!(reply_str, format!("Reply: {}", i));
        }

        join_thread(router_handle, Duration::from_secs(5), "router thread").await;
    }

    // =========================================================================
    // Test 3: Multiple DEALERs to one ROUTER (load balancing)
    // =========================================================================

    #[async_rt::test]
    async fn test_our_router_multiple_their_dealers() {
        pretty_env_logger::try_init().ok();

        let (mut our_router, bind_endpoint) = setup_our_router("tcp://127.0.0.1:0").await;
        println!("Our ROUTER bound to {}", bind_endpoint);

        const NUM_DEALERS: u32 = 5;
        const MSGS_PER_DEALER: u32 = 10;

        let ctx = zmq2::Context::new();

        // Connect multiple dealers
        let mut dealer_handles = Vec::new();
        for d in 0..NUM_DEALERS {
            let identity = format!("dealer-{}", d);
            let their_dealer = ctx.socket(zmq2::DEALER).expect("Couldn't make dealer");
            their_dealer
                .set_identity(identity.as_bytes())
                .expect("Failed to set identity");
            their_dealer
                .connect(&bind_endpoint)
                .expect("Failed to connect");

            let handle = std::thread::spawn(move || {
                for i in 0..MSGS_PER_DEALER {
                    their_dealer
                        .send(&format!("{}:{}", identity, i), 0)
                        .expect("Failed to send");

                    let reply = their_dealer
                        .recv_string(0)
                        .expect("Failed to recv")
                        .expect("Invalid UTF8");
                    assert_eq!(reply, format!("ack-{}:{}", identity, i));
                }
            });
            dealer_handles.push(handle);
        }

        // Allow connections
        async_rt::task::sleep(Duration::from_millis(200)).await;

        // Router handles all messages
        for _ in 0..(NUM_DEALERS * MSGS_PER_DEALER) {
            let msg = our_router.recv().await.expect("Failed to recv");
            assert_eq!(msg.len(), 2);

            let identity = msg.get(0).unwrap().clone();
            let payload = String::from_utf8(msg.get(1).unwrap().to_vec()).unwrap();

            // Reply with ack
            let mut reply = ZmqMessage::from(format!("ack-{}", payload));
            reply.push_front(identity);
            our_router.send(reply).await.expect("Failed to send");
        }

        for handle in dealer_handles {
            join_thread(handle, Duration::from_secs(5), "dealer thread").await;
        }
    }
}
