use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::{Endpoint, SocketOptions, ZmqError, ZmqMessage};

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn unique_ipc_endpoint(name: &str) -> (String, PathBuf) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("z-{name}-{}-{nanos}.sock", std::process::id()));
    (format!("ipc://{}", path.display()), path)
}

#[async_rt::test]
async fn ipc_connect_before_bind_retries_until_bind() {
    let (endpoint, path) = unique_ipc_endpoint("connect-before-bind");
    let subscriber_endpoint = endpoint.clone();
    let payload = b"connect-before-bind".to_vec();

    let (connected_tx, connected_rx) = futures::channel::oneshot::channel();
    let (received_tx, received_rx) = futures::channel::oneshot::channel();
    async_rt::task::spawn(async move {
        let mut sub_socket = zeromq::SubSocket::new();
        sub_socket.subscribe("").await.unwrap();
        sub_socket.connect(&subscriber_endpoint).await.unwrap();
        let _ = connected_tx.send(());

        let message = async_rt::task::timeout(Duration::from_secs(2), sub_socket.recv())
            .await
            .expect("timeout waiting for message")
            .unwrap();
        let _ = received_tx.send(message.get(0).unwrap().to_vec());
    });

    async_rt::task::sleep(Duration::from_millis(100)).await;

    let mut pub_socket = zeromq::PubSocket::new();
    pub_socket.bind(&endpoint).await.unwrap();

    async_rt::task::timeout(Duration::from_secs(5), connected_rx)
        .await
        .expect("timeout waiting for connect")
        .expect("connect task dropped");

    async_rt::task::sleep(Duration::from_millis(100)).await;
    pub_socket
        .send(ZmqMessage::from(payload.clone()))
        .await
        .unwrap();

    let received = async_rt::task::timeout(Duration::from_secs(2), received_rx)
        .await
        .expect("timeout waiting for received payload")
        .expect("receiver task dropped");
    assert_eq!(received, payload);

    let errs = pub_socket.close().await;
    assert!(errs.is_empty(), "Could not unbind socket: {:?}", errs);
    let _ = std::fs::remove_file(path);
}

#[async_rt::test]
async fn connect_timeout_expires_for_missing_ipc_socket() {
    let (endpoint, _path) = unique_ipc_endpoint("missing");
    let mut options = SocketOptions::default();
    options.connect_timeout(Duration::from_millis(50));

    let mut socket = zeromq::DealerSocket::with_options(options);
    let err = socket
        .connect(&endpoint)
        .await
        .expect_err("connect should time out");

    assert!(matches!(err, ZmqError::ConnectTimeout(_)), "{err:?}");
}

#[async_rt::test]
async fn tcp_active_bind_to_same_endpoint_is_rejected() {
    let mut first = zeromq::RouterSocket::new();
    let bound = first.bind("tcp://127.0.0.1:0").await.unwrap();
    let endpoint = bound.to_string();

    let mut second = zeromq::RouterSocket::new();
    let err = second
        .bind(&endpoint)
        .await
        .expect_err("second bind should fail while first listener is active");

    assert!(matches!(err, ZmqError::Network(_)), "{err:?}");

    let errs = first.close().await;
    assert!(errs.is_empty(), "Could not unbind first socket: {:?}", errs);
}

#[async_rt::test]
async fn ipc_active_bind_to_same_path_is_rejected() {
    let (endpoint, path) = unique_ipc_endpoint("active-duplicate-bind");

    let mut first = zeromq::RouterSocket::new();
    let first_bound = first.bind(&endpoint).await.unwrap();
    assert_eq!(first_bound.to_string(), endpoint);

    let mut second = zeromq::RouterSocket::new();
    let err = second
        .bind(&endpoint)
        .await
        .expect_err("second bind should fail while first listener is active");

    assert!(matches!(err, ZmqError::Network(_)), "{err:?}");

    let errs = first.close().await;
    assert!(errs.is_empty(), "Could not unbind first socket: {:?}", errs);
    let _ = std::fs::remove_file(path);
}

#[async_rt::test]
async fn ipc_close_allows_rebinding_same_path() {
    let (endpoint, path) = unique_ipc_endpoint("rebind");

    let mut first = zeromq::RouterSocket::new();
    let first_bound = first.bind(&endpoint).await.unwrap();
    assert_eq!(first_bound.to_string(), endpoint);

    let errs = first.close().await;
    assert!(errs.is_empty(), "Could not unbind first socket: {:?}", errs);

    let mut second = zeromq::RouterSocket::new();
    let second_bound = second.bind(&endpoint).await.unwrap();
    assert_eq!(second_bound.to_string(), endpoint);

    let errs = second.close().await;
    assert!(
        errs.is_empty(),
        "Could not unbind second socket: {:?}",
        errs
    );
    let _ = std::fs::remove_file(path);
}

#[async_rt::test]
async fn unbind_unbound_endpoint_returns_no_such_bind() {
    let mut socket = zeromq::RouterSocket::new();
    let endpoint: Endpoint = "tcp://127.0.0.1:1".parse().unwrap();

    let err = socket
        .unbind(endpoint.clone())
        .await
        .expect_err("unbinding an endpoint that was never bound should fail");

    assert!(matches!(err, ZmqError::NoSuchBind(e) if e == endpoint));
}

#[async_rt::test]
async fn no_connect_timeout_allows_delayed_ipc_bind() {
    let (endpoint, path) = unique_ipc_endpoint("no-timeout");
    let dealer_endpoint = endpoint.clone();

    let (connected_tx, connected_rx) = futures::channel::oneshot::channel();
    async_rt::task::spawn(async move {
        let mut options = SocketOptions::default();
        options.no_connect_timeout();
        let mut dealer = zeromq::DealerSocket::with_options(options);
        dealer.connect(&dealer_endpoint).await.unwrap();
        let _ = connected_tx.send(());
    });

    async_rt::task::sleep(Duration::from_millis(100)).await;

    let mut router = zeromq::RouterSocket::new();
    router.bind(&endpoint).await.unwrap();

    async_rt::task::timeout(Duration::from_secs(5), connected_rx)
        .await
        .expect("timeout waiting for delayed IPC connect")
        .expect("connect task dropped");

    let errs = router.close().await;
    assert!(errs.is_empty(), "Could not unbind socket: {:?}", errs);
    let _ = std::fs::remove_file(path);
}
