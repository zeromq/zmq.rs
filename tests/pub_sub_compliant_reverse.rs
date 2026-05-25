//! Conformance tests for our PUB with their SUB (reverse of `pub_sub_compliant.rs`).
//!
//! Tests that our zmq.rs PUB socket correctly broadcasts to libzmq SUB sockets.

mod compliance;
use compliance::join_thread;

use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::ZmqMessage;

use std::time::Duration;

async fn setup_our_pub(bind_endpoint: &str) -> (zeromq::PubSocket, String) {
    let mut our_pub = zeromq::PubSocket::new();
    let endpoint = our_pub.bind(bind_endpoint).await.expect("Failed to bind");
    (our_pub, endpoint.to_string())
}

fn setup_their_subs(
    ctx: &zmq2::Context,
    connect_endpoint: &str,
    n_subs: usize,
    subscription: &[u8],
) -> Vec<zmq2::Socket> {
    let mut subs = Vec::new();
    for _ in 0..n_subs {
        let their_sub = ctx.socket(zmq2::SUB).expect("Couldn't make sub socket");
        their_sub.set_ipv6(true).expect("Failed to enable IPV6");
        their_sub
            .connect(connect_endpoint)
            .expect("Failed to connect");
        their_sub
            .set_subscribe(subscription)
            .expect("Failed to subscribe");
        subs.push(their_sub);
    }
    subs
}

#[cfg(test)]
mod test {
    use super::*;

    #[async_rt::test]
    async fn test_our_pub_their_sub() {
        pretty_env_logger::try_init().ok();

        const N_SUBS: usize = 4;

        async fn do_test(our_endpoint: &str) {
            let (mut our_pub, bind_endpoint) = setup_our_pub(our_endpoint).await;
            println!("Our PUB bound to {}", bind_endpoint);

            let ctx = zmq2::Context::new();
            let their_subs = setup_their_subs(&ctx, &bind_endpoint, N_SUBS, b"");

            // Set receive timeout to avoid blocking forever
            for sub in &their_subs {
                sub.set_rcvtimeo(2000).expect("Failed to set timeout");
            }

            // Slow joiner: wait for subscriptions to propagate
            async_rt::task::sleep(Duration::from_millis(300)).await;

            const NUM_MSGS: u32 = 16;

            // Run their subs in threads - collect messages with timeout
            let sub_handles: Vec<_> = their_subs
                .into_iter()
                .enumerate()
                .map(|(idx, sub)| {
                    std::thread::spawn(move || {
                        let mut received = Vec::new();
                        while let Ok(Ok(msg)) = sub.recv_string(0) {
                            received.push(msg);
                        }
                        (idx, received)
                    })
                })
                .collect();

            // Our pub sends
            for i in 0..NUM_MSGS {
                let msg = ZmqMessage::from(format!("Message: {}", i));
                our_pub.send(msg).await.expect("Failed to send");
            }

            // Join all subscriber threads and verify they received messages
            for handle in sub_handles {
                let (idx, received) =
                    join_thread(handle, Duration::from_secs(5), "subscriber receiver").await;
                // Each sub should receive at least some messages (slow joiner may miss early ones)
                assert!(!received.is_empty(), "Sub {} received no messages", idx);
                println!("Sub {} received {} messages", idx, received.len());
            }
        }

        let endpoints = vec![
            "tcp://127.0.0.1:0",
            "tcp://[::1]:0",
            "ipc://our_pub_test.sock",
        ];

        for e in endpoints {
            println!("Testing with endpoint {}", e);
            do_test(e).await;

            // Clean up IPC socket files
            if let Some(path) = e.strip_prefix("ipc://") {
                std::fs::remove_file(path).ok();
            }
        }
    }

    #[async_rt::test]
    async fn test_our_pub_their_sub_topic_filtering() {
        pretty_env_logger::try_init().ok();

        let (mut our_pub, bind_endpoint) = setup_our_pub("tcp://127.0.0.1:0").await;
        println!("Our PUB bound to {}", bind_endpoint);

        let ctx = zmq2::Context::new();

        // Create subs with different topic filters
        let topic1_sub = ctx.socket(zmq2::SUB).expect("Couldn't make sub");
        topic1_sub
            .connect(&bind_endpoint)
            .expect("Failed to connect");
        topic1_sub
            .set_subscribe(b"topic1")
            .expect("Failed to subscribe");

        let topic2_sub = ctx.socket(zmq2::SUB).expect("Couldn't make sub");
        topic2_sub
            .connect(&bind_endpoint)
            .expect("Failed to connect");
        topic2_sub
            .set_subscribe(b"topic2")
            .expect("Failed to subscribe");

        let all_sub = ctx.socket(zmq2::SUB).expect("Couldn't make sub");
        all_sub.connect(&bind_endpoint).expect("Failed to connect");
        all_sub.set_subscribe(b"").expect("Failed to subscribe");

        // Wait for subscriptions
        async_rt::task::sleep(Duration::from_millis(200)).await;

        // Spawn receiver threads with RCVTIMEO to avoid blocking forever
        topic1_sub
            .set_rcvtimeo(1000)
            .expect("Failed to set timeout");
        topic2_sub
            .set_rcvtimeo(1000)
            .expect("Failed to set timeout");
        all_sub.set_rcvtimeo(1000).expect("Failed to set timeout");

        let topic1_handle = std::thread::spawn(move || {
            let mut received = Vec::new();
            while let Ok(msg) = topic1_sub.recv_string(0) {
                received.push(msg.unwrap());
            }
            received
        });

        let topic2_handle = std::thread::spawn(move || {
            let mut received = Vec::new();
            while let Ok(msg) = topic2_sub.recv_string(0) {
                received.push(msg.unwrap());
            }
            received
        });

        let all_handle = std::thread::spawn(move || {
            let mut received = Vec::new();
            while let Ok(msg) = all_sub.recv_string(0) {
                received.push(msg.unwrap());
            }
            received
        });

        // Send messages with different topics
        for msg in &[
            "topic1-message-a",
            "topic2-message-b",
            "topic1-message-c",
            "other-message-d",
            "topic2-message-e",
        ] {
            our_pub
                .send(ZmqMessage::from(*msg))
                .await
                .expect("Failed to send");
        }

        // Wait and check results
        let topic1_msgs =
            join_thread(topic1_handle, Duration::from_secs(3), "topic1 receiver").await;
        let topic2_msgs =
            join_thread(topic2_handle, Duration::from_secs(3), "topic2 receiver").await;
        let all_msgs = join_thread(all_handle, Duration::from_secs(3), "all receiver").await;

        assert_eq!(
            topic1_msgs,
            vec!["topic1-message-a", "topic1-message-c"],
            "topic1 sub should only receive topic1 messages"
        );
        assert_eq!(
            topic2_msgs,
            vec!["topic2-message-b", "topic2-message-e"],
            "topic2 sub should only receive topic2 messages"
        );
        assert_eq!(all_msgs.len(), 5, "all sub should receive all messages");
    }
}
