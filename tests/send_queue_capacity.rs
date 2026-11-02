#![cfg(feature = "tokio-runtime")]

use bytes::Bytes;
use zeromq::prelude::*;
use zeromq::{PushSocket, SocketOptions, ZmqMessage};

use std::time::Duration;

const MESSAGE_SIZE: usize = 64 * 1024;
const ATTEMPTS: usize = 2_000;
const STALL: Duration = Duration::from_millis(500);

/// Sends until one send waits longer than `STALL` and returns how many were accepted,
/// or `None` when every attempt was accepted.
async fn accepted_before_stall(push: &mut PushSocket) -> Option<usize> {
    let payload = Bytes::from(vec![b'x'; MESSAGE_SIZE]);
    for accepted in 0..ATTEMPTS {
        let send = push.send(ZmqMessage::from(payload.clone()));
        match tokio::time::timeout(STALL, send).await {
            Ok(result) => result.expect("send failed"),
            Err(_) => return Some(accepted),
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_small_send_queue_holds_back_a_push_to_a_peer_that_does_not_read() {
    let mut options = SocketOptions::default();
    options.send_queue_capacity(8);
    let mut push = PushSocket::with_options(options);
    let endpoint = push.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let mut pull = zeromq::PullSocket::new();
    pull.connect(&endpoint).await.unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;

    // The queue holds 8 messages; the rest of what is accepted sits in the writer's
    // frame buffer and the kernel's socket buffers, which hold a few MiB at most.
    let accepted = accepted_before_stall(&mut push)
        .await
        .expect("every send was accepted, so the queue applied no backpressure");
    assert!(
        accepted < ATTEMPTS / 2,
        "{accepted} messages of {MESSAGE_SIZE} bytes were accepted before the first wait"
    );

    // The held-back send completes once the peer reads.
    let reader = tokio::spawn(async move {
        loop {
            if pull.recv().await.is_err() {
                break;
            }
        }
    });
    let payload = Bytes::from(vec![b'y'; MESSAGE_SIZE]);
    tokio::time::timeout(Duration::from_secs(5), push.send(ZmqMessage::from(payload)))
        .await
        .expect("a send stayed blocked after the peer started reading")
        .expect("send failed");
    reader.abort();
}

#[test]
#[should_panic(expected = "send queue capacity must be positive")]
fn a_zero_send_queue_capacity_is_rejected() {
    SocketOptions::default().send_queue_capacity(0);
}
