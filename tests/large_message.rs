use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::{Endpoint, ZmqMessage};

use bytes::Bytes;
use std::convert::TryFrom;
use std::time::Duration;

fn tcp_endpoint(endpoint: Endpoint) -> String {
    match endpoint {
        Endpoint::Tcp(_, port) => format!("tcp://127.0.0.1:{port}"),
        _ => unreachable!("expected tcp endpoint"),
    }
}

#[async_rt::test]
async fn pub_sub_delivers_large_message() {
    let payload = vec![0xAB; 300_000];

    let mut sub_socket = zeromq::SubSocket::new();
    let endpoint = tcp_endpoint(sub_socket.bind("tcp://127.0.0.1:0").await.unwrap());
    sub_socket.subscribe("").await.unwrap();

    let mut pub_socket = zeromq::PubSocket::new();
    pub_socket.connect(&endpoint).await.unwrap();
    async_rt::task::sleep(Duration::from_millis(100)).await;

    pub_socket
        .send(ZmqMessage::from(payload.clone()))
        .await
        .unwrap();

    let received = async_rt::task::timeout(Duration::from_secs(2), sub_socket.recv())
        .await
        .expect("timeout waiting for large PUB message")
        .unwrap();
    assert_eq!(received.get(0).unwrap().as_ref(), payload.as_slice());
}

#[async_rt::test]
async fn xpub_sub_delivers_large_message() {
    let payload = vec![0xCD; 300_000];

    let mut xpub_socket = zeromq::XPubSocket::new();
    let endpoint = tcp_endpoint(xpub_socket.bind("tcp://127.0.0.1:0").await.unwrap());

    let mut sub_socket = zeromq::SubSocket::new();
    sub_socket.connect(&endpoint).await.unwrap();
    sub_socket.subscribe("").await.unwrap();

    let _subscription = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
        .await
        .expect("timeout waiting for subscription")
        .unwrap();

    xpub_socket
        .send(ZmqMessage::from(payload.clone()))
        .await
        .unwrap();

    let received = async_rt::task::timeout(Duration::from_secs(2), sub_socket.recv())
        .await
        .expect("timeout waiting for large XPUB message")
        .unwrap();
    assert_eq!(received.get(0).unwrap().as_ref(), payload.as_slice());
}

#[async_rt::test]
async fn xsub_xpub_delivers_large_message() {
    let payload = vec![0xEF; 300_000];

    let mut xpub_socket = zeromq::XPubSocket::new();
    let endpoint = tcp_endpoint(xpub_socket.bind("tcp://127.0.0.1:0").await.unwrap());

    let mut xsub_socket = zeromq::XSubSocket::new();
    xsub_socket.connect(&endpoint).await.unwrap();
    async_rt::task::sleep(Duration::from_millis(100)).await;

    xsub_socket
        .send(ZmqMessage::from(payload.clone()))
        .await
        .unwrap();

    let received = async_rt::task::timeout(Duration::from_secs(2), xpub_socket.recv())
        .await
        .expect("timeout waiting for large XSUB message")
        .unwrap();
    assert_eq!(received.get(0).unwrap().as_ref(), payload.as_slice());
}

#[async_rt::test]
async fn dealer_router_delivers_large_multipart_message() {
    let small = Bytes::from_static(b"metadata");
    let large_a = Bytes::from(vec![0xA1; 300_000]);
    let large_b = Bytes::from(vec![0xB2; 128_000]);

    let mut router_socket = zeromq::RouterSocket::new();
    let endpoint = tcp_endpoint(router_socket.bind("tcp://127.0.0.1:0").await.unwrap());

    let mut dealer_socket = zeromq::DealerSocket::new();
    dealer_socket.connect(&endpoint).await.unwrap();
    async_rt::task::sleep(Duration::from_millis(100)).await;

    let sent = ZmqMessage::try_from(vec![small.clone(), large_a.clone(), large_b.clone()])
        .expect("multipart message");
    dealer_socket.send(sent).await.unwrap();

    let received = async_rt::task::timeout(Duration::from_secs(2), router_socket.recv())
        .await
        .expect("timeout waiting for large multipart DEALER message")
        .unwrap();

    assert_eq!(received.len(), 4);
    assert_eq!(received.get(1).unwrap(), &small);
    assert_eq!(received.get(2).unwrap(), &large_a);
    assert_eq!(received.get(3).unwrap(), &large_b);
}
