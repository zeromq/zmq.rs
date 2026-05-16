use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::ZmqMessage;

use std::time::Duration;

#[async_rt::test]
async fn pull_recv_timeout_then_cached_drain_preserves_per_peer_order() {
    let mut pull = zeromq::PullSocket::new();
    let endpoint = pull.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let mut push_a = zeromq::PushSocket::new();
    push_a.connect(&endpoint).await.unwrap();
    let mut push_b = zeromq::PushSocket::new();
    push_b.connect(&endpoint).await.unwrap();
    async_rt::task::sleep(Duration::from_millis(100)).await;

    let cancelled = async_rt::task::timeout(Duration::from_millis(10), pull.recv()).await;
    assert!(
        cancelled.is_err(),
        "empty PULL recv should be cancelled by the timeout before messages are sent"
    );

    for i in 0..8 {
        push_a
            .send(ZmqMessage::from(format!("a:{i}")))
            .await
            .unwrap();
        push_b
            .send(ZmqMessage::from(format!("b:{i}")))
            .await
            .unwrap();
    }

    let mut seen_a = Vec::new();
    let mut seen_b = Vec::new();
    for _ in 0..16 {
        let message = async_rt::task::timeout(Duration::from_secs(2), pull.recv())
            .await
            .expect("timed out waiting for queued PULL message")
            .unwrap();
        let payload = String::from_utf8(message.get(0).unwrap().to_vec()).unwrap();
        let (peer, seq) = payload.split_once(':').unwrap();
        let seq = seq.parse::<usize>().unwrap();
        match peer {
            "a" => seen_a.push(seq),
            "b" => seen_b.push(seq),
            other => panic!("unexpected peer marker: {other}"),
        }
    }

    assert_eq!(seen_a, (0..8).collect::<Vec<_>>());
    assert_eq!(seen_b, (0..8).collect::<Vec<_>>());
}
