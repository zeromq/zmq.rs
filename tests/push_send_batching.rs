use bytes::Bytes;
use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::ZmqMessage;

use std::convert::TryFrom;
use std::time::Duration;

const CONNECT_DELAY: Duration = Duration::from_millis(250);
const RECV_TIMEOUT: Duration = Duration::from_secs(5);

#[async_rt::test]
async fn push_batched_send_drains_queued_messages_after_drop() {
    const MESSAGES: usize = 512;

    let mut pull = zeromq::PullSocket::new();
    let endpoint = pull.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let mut push = zeromq::PushSocket::new();
    push.connect(&endpoint).await.unwrap();
    async_rt::task::sleep(CONNECT_DELAY).await;

    for seq in 0..MESSAGES {
        push.send(single_frame(seq)).await.unwrap();
    }

    drop(push);

    for expected in 0..MESSAGES {
        let message = recv_with_timeout(&mut pull, "queued PUSH message").await;
        assert_eq!(parse_seq(&message), expected);
    }
}

#[async_rt::test]
async fn push_batched_send_preserves_multipart_frames() {
    const MESSAGES: usize = 128;

    let mut pull = zeromq::PullSocket::new();
    let endpoint = pull.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let mut push = zeromq::PushSocket::new();
    push.connect(&endpoint).await.unwrap();
    async_rt::task::sleep(CONNECT_DELAY).await;

    for seq in 0..MESSAGES {
        push.send(multipart(seq)).await.unwrap();
    }

    for expected in 0..MESSAGES {
        let message = recv_with_timeout(&mut pull, "multipart PUSH message").await;
        assert_eq!(message.len(), 3);
        assert_eq!(message.get(0).unwrap().as_ref(), b"meta");
        assert_eq!(
            message.get(1).unwrap().as_ref(),
            format!("seq:{expected:04}").as_bytes()
        );
        assert_eq!(message.get(2).unwrap().as_ref(), vec![0xAB; 300].as_slice());
    }
}

#[async_rt::test]
async fn push_batched_send_preserves_round_robin_distribution() {
    const MESSAGES: usize = 128;

    let mut pull_a = zeromq::PullSocket::new();
    let endpoint_a = pull_a.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let mut pull_b = zeromq::PullSocket::new();
    let endpoint_b = pull_b.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let mut push = zeromq::PushSocket::new();
    push.connect(&endpoint_a).await.unwrap();
    push.connect(&endpoint_b).await.unwrap();
    async_rt::task::sleep(CONNECT_DELAY).await;

    let recv_a = async_rt::task::spawn(collect_sequences(pull_a, MESSAGES / 2));
    let recv_b = async_rt::task::spawn(collect_sequences(pull_b, MESSAGES / 2));

    for seq in 0..MESSAGES {
        push.send(single_frame(seq)).await.unwrap();
    }

    let seqs_a = recv_a.await.unwrap();
    let seqs_b = recv_b.await.unwrap();
    assert_strict_alternating(&seqs_a);
    assert_strict_alternating(&seqs_b);

    let mut all = seqs_a;
    all.extend(seqs_b);
    all.sort_unstable();
    assert_eq!(all, (0..MESSAGES).collect::<Vec<_>>());
}

async fn collect_sequences(mut pull: zeromq::PullSocket, count: usize) -> Vec<usize> {
    let mut seqs = Vec::with_capacity(count);
    for _ in 0..count {
        let message = recv_with_timeout(&mut pull, "round-robin message").await;
        seqs.push(parse_seq(&message));
    }
    seqs
}

async fn recv_with_timeout(pull: &mut zeromq::PullSocket, label: &'static str) -> ZmqMessage {
    async_rt::task::timeout(RECV_TIMEOUT, pull.recv())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {label}"))
        .expect("PULL recv failed")
}

fn assert_strict_alternating(seqs: &[usize]) {
    for window in seqs.windows(2) {
        assert_eq!(window[1], window[0] + 2);
    }
}

fn single_frame(seq: usize) -> ZmqMessage {
    ZmqMessage::from(format!("seq:{seq:04}"))
}

fn multipart(seq: usize) -> ZmqMessage {
    ZmqMessage::try_from(vec![
        Bytes::from_static(b"meta"),
        Bytes::from(format!("seq:{seq:04}")),
        Bytes::from(vec![0xAB; 300]),
    ])
    .unwrap()
}

fn parse_seq(message: &ZmqMessage) -> usize {
    let frame = message.get(0).expect("single frame");
    let text = std::str::from_utf8(frame.as_ref()).expect("utf8 frame");
    text.strip_prefix("seq:").unwrap().parse().unwrap()
}
