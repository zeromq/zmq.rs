use bytes::Bytes;
use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::{Endpoint, ZmqMessage};

use std::time::Duration;

fn tcp_endpoint(endpoint: Endpoint) -> String {
    match endpoint {
        Endpoint::Tcp(_, port) => format!("tcp://127.0.0.1:{port}"),
        _ => unreachable!("expected tcp endpoint"),
    }
}

#[async_rt::test]
async fn push_pull_delivers_large_multipart_message() {
    let topic = Bytes::from_static(b"topic");
    let large = Bytes::from(vec![0xAB; 300_000]);
    let tail = Bytes::from(vec![0xCD; 4_096]);

    let mut pull = zeromq::PullSocket::new();
    let endpoint = tcp_endpoint(pull.bind("tcp://127.0.0.1:0").await.unwrap());

    let mut push = zeromq::PushSocket::new();
    push.connect(&endpoint).await.unwrap();
    async_rt::task::sleep(Duration::from_millis(100)).await;

    let message = ZmqMessage::try_from(vec![topic.clone(), large.clone(), tail.clone()]).unwrap();
    push.send(message).await.unwrap();

    let received = async_rt::task::timeout(Duration::from_secs(2), pull.recv())
        .await
        .expect("timeout waiting for PUSH/PULL multipart message")
        .unwrap();
    assert_eq!(received.len(), 3);
    assert_eq!(received.get(0), Some(&topic));
    assert_eq!(received.get(1), Some(&large));
    assert_eq!(received.get(2), Some(&tail));
}

#[async_rt::test]
async fn dealer_router_roundtrips_large_multipart_message() {
    let header = Bytes::from_static(b"header");
    let large = Bytes::from(vec![0xEF; 300_000]);
    let tail = Bytes::from(vec![0x42; 4_096]);

    let mut router = zeromq::RouterSocket::new();
    let endpoint = tcp_endpoint(router.bind("tcp://127.0.0.1:0").await.unwrap());

    let mut dealer = zeromq::DealerSocket::new();
    dealer.connect(&endpoint).await.unwrap();
    async_rt::task::sleep(Duration::from_millis(100)).await;

    let message = ZmqMessage::try_from(vec![header.clone(), large.clone(), tail.clone()]).unwrap();
    dealer.send(message).await.unwrap();

    let routed = async_rt::task::timeout(Duration::from_secs(2), router.recv())
        .await
        .expect("timeout waiting for DEALER/ROUTER multipart message")
        .unwrap();
    assert_eq!(routed.len(), 4);
    assert!(!routed.get(0).unwrap().is_empty());
    assert_eq!(routed.get(1), Some(&header));
    assert_eq!(routed.get(2), Some(&large));
    assert_eq!(routed.get(3), Some(&tail));

    router.send(routed).await.unwrap();

    let echoed = async_rt::task::timeout(Duration::from_secs(2), dealer.recv())
        .await
        .expect("timeout waiting for ROUTER/DEALER multipart echo")
        .unwrap();
    assert_eq!(echoed.len(), 3);
    assert_eq!(echoed.get(0), Some(&header));
    assert_eq!(echoed.get(1), Some(&large));
    assert_eq!(echoed.get(2), Some(&tail));
}
