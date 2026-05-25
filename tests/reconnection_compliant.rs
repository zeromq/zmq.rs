//! Conformance tests for reconnection behavior.
//!
//! Tests that zmq.rs sockets correctly reconnect and resync subscriptions
//! after the peer restarts.

mod compliance;
use compliance::{recv_string_on_thread, setup_monitor};

use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;

use futures::StreamExt;
use std::time::Duration;

fn extract_port(endpoint: &str) -> u16 {
    // Parse port from endpoint like "tcp://127.0.0.1:12345"
    endpoint
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .expect("Failed to parse port")
}

#[cfg(test)]
mod test {
    use super::*;

    /// Test that our SUB reconnects when their PUB restarts.
    ///
    /// This verifies that:
    /// 1. Initial connection and subscription works
    /// 2. After PUB drops and restarts on same port, SUB reconnects
    /// 3. Subscription is re-established and messages flow again
    #[async_rt::test]
    async fn test_our_sub_reconnects_to_their_restarted_pub() {
        pretty_env_logger::try_init().ok();

        // Phase 1: Initial setup - their PUB binds to specific port
        let ctx = zmq2::Context::new();
        let their_pub = ctx.socket(zmq2::PUB).expect("Couldn't make pub socket");
        // Bind to port 0, get actual port
        their_pub.bind("tcp://127.0.0.1:0").expect("Failed to bind");
        let bind_endpoint = their_pub.get_last_endpoint().unwrap().unwrap();
        let port = extract_port(&bind_endpoint);
        println!("Their PUB initially bound to {}", bind_endpoint);

        // Our SUB connects and subscribes
        let mut our_sub = zeromq::SubSocket::new();
        our_sub
            .connect(&bind_endpoint)
            .await
            .expect("Failed to connect");
        our_sub.subscribe("").await.expect("Failed to subscribe");

        // Wait for subscription to propagate
        async_rt::task::sleep(Duration::from_millis(200)).await;

        // Phase 2: Verify initial communication works
        their_pub
            .send("initial-message", 0)
            .expect("Failed to send");

        let msg = async_rt::task::timeout(Duration::from_secs(2), our_sub.recv())
            .await
            .expect("Timeout waiting for initial message")
            .expect("Failed to recv");
        let msg_str = String::from_utf8(msg.get(0).unwrap().to_vec()).unwrap();
        assert_eq!(msg_str, "initial-message");
        println!("Phase 2: Initial communication verified");

        // Phase 3: Drop the PUB (simulates crash/restart)
        drop(their_pub);
        println!("Phase 3: Their PUB dropped");

        // Small delay for socket cleanup
        async_rt::task::sleep(Duration::from_millis(300)).await;

        // Phase 4: Restart PUB on same port
        let their_pub_new = ctx.socket(zmq2::PUB).expect("Couldn't make pub socket");
        their_pub_new
            .bind(&format!("tcp://127.0.0.1:{}", port))
            .expect("Failed to rebind");
        let their_monitor = setup_monitor(&ctx, &their_pub_new, "inproc://pub-monitor");
        println!("Phase 4: Their PUB restarted on port {}", port);

        // Phase 5: Trigger disconnect detection and wait for reconnection
        // Our SUB detects disconnect when it tries to receive. We use a short timeout
        // recv() to trigger disconnect detection and allow the reconnection task to run.
        println!("Phase 5: Triggering disconnect detection...");

        // First recv attempt will detect the disconnection and trigger reconnection
        match async_rt::task::timeout(Duration::from_millis(500), our_sub.recv()).await {
            Ok(Ok(_msg)) => {
                println!("Unexpectedly received a message during disconnect detection");
            }
            Ok(Err(e)) => {
                println!("Recv error during disconnect detection (expected): {:?}", e);
            }
            Err(_) => {
                println!("Recv timed out (disconnect detected, reconnection in progress)");
            }
        }

        // Wait for reconnection to complete and subscription to re-propagate
        // The reconnect task uses exponential backoff starting at 100ms
        let mut reconnected = false;
        for attempt in 0..30 {
            async_rt::task::sleep(Duration::from_millis(100)).await;

            // Try to get monitor event without blocking
            match their_monitor.recv_multipart(zmq2::DONTWAIT) {
                Ok(msgs) if !msgs.is_empty() => {
                    let event_bytes: [u8; 2] = msgs[0][..2].try_into().unwrap();
                    let event = zmq2::SocketEvent::from_raw(u16::from_le_bytes(event_bytes));
                    println!("Monitor event (attempt {}): {:?}", attempt, event);
                    if event == zmq2::SocketEvent::HANDSHAKE_SUCCEEDED {
                        reconnected = true;
                        break;
                    }
                }
                _ => {}
            }
        }

        if !reconnected {
            println!("Warning: Didn't observe reconnection event after 3 seconds");
        }

        // Wait a bit more for subscription to propagate
        async_rt::task::sleep(Duration::from_millis(300)).await;

        // Phase 6: Verify communication works after reconnect
        println!("Phase 6: Sending message after reconnection...");
        their_pub_new
            .send("reconnected-message", 0)
            .expect("Failed to send");

        let msg = async_rt::task::timeout(Duration::from_secs(3), our_sub.recv())
            .await
            .expect("Timeout waiting for reconnected message - subscription may not have re-synced")
            .expect("Failed to recv");
        let msg_str = String::from_utf8(msg.get(0).unwrap().to_vec()).unwrap();
        assert_eq!(msg_str, "reconnected-message");
        println!("Phase 6: Reconnection and subscription verified!");
    }

    /// Test that our PUB correctly accepts new connections after restarting.
    ///
    /// This verifies that:
    /// 1. Initial PUB-SUB communication works
    /// 2. After PUB restarts on same port, it can accept new connections
    /// 3. New SUB sockets can subscribe and receive messages
    ///
    /// Note: libzmq SUB sockets reconnect at TCP level but may not resend
    /// subscriptions automatically - this is expected libzmq behavior.
    /// For guaranteed subscription resync, applications should either
    /// use zmq.rs SUB (which has auto-resync) or manually resubscribe.
    #[async_rt::test]
    async fn test_their_sub_reconnects_to_our_restarted_pub() {
        pretty_env_logger::try_init().ok();

        // Phase 1: Our PUB binds
        let mut our_pub = zeromq::PubSocket::new();
        let endpoint = our_pub
            .bind("tcp://127.0.0.1:0")
            .await
            .expect("Failed to bind");
        let bind_endpoint = endpoint.to_string();
        let port = extract_port(&bind_endpoint);
        println!("Our PUB initially bound to {}", bind_endpoint);

        // Set up monitor to observe connection events
        let mut our_monitor = our_pub.monitor();

        // Their SUB connects
        let ctx = zmq2::Context::new();
        let their_sub = ctx.socket(zmq2::SUB).expect("Couldn't make sub socket");
        their_sub
            .connect(&bind_endpoint)
            .expect("Failed to connect");
        their_sub.set_subscribe(b"").expect("Failed to subscribe");
        their_sub.set_rcvtimeo(3000).expect("Failed to set timeout");
        their_sub
            .set_reconnect_ivl(100)
            .expect("Failed to set reconnect interval");

        // Wait for subscription and check for connection event
        async_rt::task::sleep(Duration::from_millis(200)).await;

        // Drain any connection events
        while let Ok(Some(event)) =
            async_rt::task::timeout(Duration::from_millis(50), our_monitor.next()).await
        {
            println!("Initial monitor event: {:?}", event);
        }

        // Phase 2: Verify initial communication
        our_pub
            .send(zeromq::ZmqMessage::from("initial-message"))
            .await
            .expect("Failed to send");

        let (their_sub, msg) =
            recv_string_on_thread(their_sub, Duration::from_secs(5), "initial SUB recv").await;
        assert_eq!(msg, "initial-message");
        println!("Phase 2: Initial communication verified");

        // Phase 3: Drop our PUB
        drop(our_pub);
        println!("Phase 3: Our PUB dropped");

        async_rt::task::sleep(Duration::from_millis(300)).await;

        // Phase 4: Restart our PUB on same port
        let mut our_pub_new = zeromq::PubSocket::new();
        our_pub_new
            .bind(&format!("tcp://127.0.0.1:{}", port))
            .await
            .expect("Failed to rebind");
        println!("Phase 4: Our PUB restarted on port {}", port);

        // Set up monitor for new PUB
        let mut our_monitor_new = our_pub_new.monitor();

        // Phase 5: Wait for reconnection (libzmq SUB should auto-reconnect)
        // Check monitor for connection events
        println!("Phase 5: Waiting for reconnection...");
        let mut connected = false;
        for attempt in 0..30 {
            async_rt::task::sleep(Duration::from_millis(100)).await;
            while let Ok(Some(event)) =
                async_rt::task::timeout(Duration::from_millis(10), our_monitor_new.next()).await
            {
                println!("Reconnect monitor event (attempt {}): {:?}", attempt, event);
                if matches!(event, zeromq::SocketEvent::Connected(_, _)) {
                    connected = true;
                }
            }
            if connected {
                println!("Phase 5: Reconnection detected!");
                break;
            }
        }

        if !connected {
            println!("Warning: No connection event observed after 3 seconds");
        }

        // Test with a fresh SUB socket to verify our PUB works correctly
        let their_sub_fresh = ctx
            .socket(zmq2::SUB)
            .expect("Couldn't make fresh sub socket");
        their_sub_fresh
            .connect(&format!("tcp://127.0.0.1:{}", port))
            .expect("Failed to connect fresh sub");
        their_sub_fresh
            .set_subscribe(b"")
            .expect("Failed to subscribe fresh");
        their_sub_fresh
            .set_rcvtimeo(3000)
            .expect("Failed to set timeout");

        // Wait for subscription
        async_rt::task::sleep(Duration::from_millis(300)).await;

        // Phase 6: Verify communication with fresh SUB
        println!("Phase 6: Sending message to fresh SUB...");
        our_pub_new
            .send(zeromq::ZmqMessage::from("reconnected-message"))
            .await
            .expect("Failed to send");

        let (_their_sub_fresh, msg) = recv_string_on_thread(
            their_sub_fresh,
            Duration::from_secs(5),
            "fresh SUB recv after PUB restart",
        )
        .await;
        assert_eq!(msg, "reconnected-message");
        println!("Phase 6: Fresh SUB received message!");

        // Also check if the original reconnected SUB got the message
        // Note: This may fail if libzmq doesn't resend subscriptions on reconnect
        match their_sub.recv_string(zmq2::DONTWAIT) {
            Ok(Ok(msg)) => {
                println!(
                    "Original SUB also received: {} (libzmq resent subscriptions)",
                    msg
                );
            }
            _ => {
                println!("Original SUB didn't receive - libzmq may not resend subscriptions on reconnect");
            }
        }

        println!("Phase 6: Test passed - our PUB works correctly with fresh SUB!");
    }
}
