#![cfg(feature = "tokio-runtime")]

use bytes::Bytes;
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::time::{Duration, Instant};
use zeromq::prelude::*;
use zeromq::{ZmqError, ZmqMessage};

const CONNECT_DELAY: Duration = Duration::from_millis(250);
const WATCHDOG: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_recv_timeout_keeps_pending_reply_available() {
    let mut rep = zeromq::RepSocket::new();
    let endpoint = rep.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let server = tokio::spawn(async move {
        let request = rep.recv().await.unwrap();
        assert_eq!(frame_text(&request, 0), "request");

        tokio::time::sleep(Duration::from_millis(50)).await;
        rep.send(ZmqMessage::from("reply")).await.unwrap();
    });

    let mut req = zeromq::ReqSocket::new();
    req.connect(&endpoint).await.unwrap();
    tokio::time::sleep(CONNECT_DELAY).await;

    req.send(ZmqMessage::from("request")).await.unwrap();

    let timed_out = tokio::time::timeout(Duration::from_millis(5), req.recv()).await;
    assert!(
        timed_out.is_err(),
        "first recv should time out before the delayed reply"
    );

    let reply = tokio::time::timeout(Duration::from_secs(2), req.recv())
        .await
        .expect("timed out waiting for reply after canceling recv")
        .expect("recv failed after canceling a pending REQ receive");
    assert_eq!(frame_text(&reply, 0), "reply");

    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_recv_timeout_storm_preserves_single_peer_order() {
    const MESSAGES: usize = 256;

    let mut pull = zeromq::PullSocket::new();
    let endpoint = pull.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let mut push = zeromq::PushSocket::new();
    push.connect(&endpoint).await.unwrap();
    tokio::time::sleep(CONNECT_DELAY).await;

    let sender = tokio::spawn(async move {
        for seq in 0..MESSAGES {
            send_with_retry(&mut push, single_frame(seq)).await;
            if seq % 8 == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            } else {
                tokio::task::yield_now().await;
            }
        }
    });

    let deadline = Instant::now() + WATCHDOG;
    let mut expected = 0;
    let mut cancellations = 0;

    while expected < MESSAGES {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out after receiving {expected}/{MESSAGES} messages"
        );

        match tokio::time::timeout(Duration::from_micros(75).min(remaining), pull.recv()).await {
            Ok(Ok(message)) => {
                assert_eq!(message.len(), 1, "single-frame message was corrupted");
                let seq = parse_seq(frame_text(&message, 0));
                assert_eq!(seq, expected, "single-peer recv order changed");
                expected += 1;
            }
            Ok(Err(err)) => panic!("recv failed after {expected} messages: {err:?}"),
            Err(_) => cancellations += 1,
        }
    }

    sender.await.unwrap();
    assert!(
        cancellations > 0,
        "test did not exercise a canceled recv path"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_recv_cancel_storm_preserves_multipart_multi_peer_queues() {
    const PEERS: usize = 3;
    const MESSAGES_PER_PEER: usize = 96;
    const LARGE_FRAME_LEN: usize = 16 * 1024;

    let mut pull = zeromq::PullSocket::new();
    let endpoint = pull.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let mut pushes = Vec::new();
    for peer in 0..PEERS {
        let mut push = zeromq::PushSocket::new();
        push.connect(&endpoint).await.unwrap();
        pushes.push((peer, push));
    }
    tokio::time::sleep(CONNECT_DELAY).await;

    let mut senders = Vec::new();
    for (peer, mut push) in pushes {
        senders.push(tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis((peer as u64 + 1) * 10)).await;
            for seq in 0..MESSAGES_PER_PEER {
                send_with_retry(&mut push, multipart_message(peer, seq, LARGE_FRAME_LEN)).await;
                if seq % 5 == 0 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                } else {
                    tokio::task::yield_now().await;
                }
            }
        }));
    }

    let expected_total = PEERS * MESSAGES_PER_PEER;
    let mut next_by_peer = BTreeMap::new();
    for peer in 0..PEERS {
        next_by_peer.insert(peer, 0usize);
    }

    let deadline = Instant::now() + WATCHDOG;
    let mut received = 0;
    let mut cancellations = 0;

    while received < expected_total {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out after receiving {received}/{expected_total} multipart messages"
        );

        tokio::select! {
            biased;
            _ = tokio::time::sleep(Duration::from_micros(75).min(remaining)) => {
                cancellations += 1;
            }
            result = pull.recv() => {
                let message = result.unwrap_or_else(|err| {
                    panic!("recv failed after {received} multipart messages: {err:?}")
                });
                assert_multipart_message(message, &mut next_by_peer, LARGE_FRAME_LEN);
                received += 1;
            }
        }
    }

    for sender in senders {
        sender.await.unwrap();
    }

    assert!(
        cancellations > 0,
        "test did not exercise canceled recv attempts"
    );
    for peer in 0..PEERS {
        assert_eq!(
            next_by_peer[&peer], MESSAGES_PER_PEER,
            "peer {peer} did not deliver all messages"
        );
    }
}

async fn send_with_retry(socket: &mut zeromq::PushSocket, message: ZmqMessage) {
    let mut message = Some(message);
    loop {
        match socket.send(message.take().unwrap()).await {
            Ok(()) => return,
            Err(ZmqError::ReturnToSender {
                message: returned, ..
            }) => {
                message = Some(returned);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            Err(err) => panic!("send failed: {err:?}"),
        }
    }
}

fn single_frame(seq: usize) -> ZmqMessage {
    ZmqMessage::from(format!("seq:{seq:04}"))
}

fn multipart_message(peer: usize, seq: usize, large_frame_len: usize) -> ZmqMessage {
    let marker = (b'a' + peer as u8) as char;
    let large = std::iter::repeat_n(marker, large_frame_len).collect::<String>();

    ZmqMessage::try_from(vec![
        Bytes::from(format!("peer:{peer}")),
        Bytes::from(format!("seq:{seq:04}")),
        Bytes::from(large),
        Bytes::from(format!("end:{peer}:{seq:04}")),
    ])
    .unwrap()
}

fn assert_multipart_message(
    message: ZmqMessage,
    next_by_peer: &mut BTreeMap<usize, usize>,
    large_frame_len: usize,
) {
    assert_eq!(message.len(), 4, "multipart message frame count changed");

    let peer_text = frame_text(&message, 0);
    let peer = peer_text
        .strip_prefix("peer:")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let seq = parse_seq(frame_text(&message, 1));
    let expected_seq = next_by_peer
        .get_mut(&peer)
        .unwrap_or_else(|| panic!("unexpected peer {peer}"));

    assert_eq!(
        seq, *expected_seq,
        "peer {peer} message order changed or a message was dropped"
    );

    let payload = message.get(2).unwrap();
    assert_eq!(payload.len(), large_frame_len, "large frame length changed");
    assert!(
        payload.iter().all(|byte| *byte == b'a' + peer as u8),
        "large frame payload was corrupted for peer {peer}, seq {seq}"
    );
    assert_eq!(frame_text(&message, 3), format!("end:{peer}:{seq:04}"));

    *expected_seq += 1;
}

fn parse_seq(frame: String) -> usize {
    frame.strip_prefix("seq:").unwrap().parse().unwrap()
}

fn frame_text(message: &ZmqMessage, index: usize) -> String {
    String::from_utf8(message.get(index).unwrap().to_vec()).unwrap()
}
