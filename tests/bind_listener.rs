use std::time::Duration;

#[cfg(target_family = "unix")]
use std::time::{SystemTime, UNIX_EPOCH};

use futures::{AsyncReadExt, StreamExt};
use zeromq::__async_rt as async_rt;
use zeromq::prelude::{Socket, SocketRecv, SocketSend};
use zeromq::{
    DealerSocket, Endpoint, Host, RouterSocket, SocketEvent, SocketOptions, ZmqError, ZmqMessage,
};

#[async_rt::test]
async fn adopts_tcp_listener_and_accepts_connections() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let expected_addr = listener.local_addr().unwrap();

    let mut router = RouterSocket::new();
    let mut monitor = router.monitor();
    let endpoint = router.bind_listener(listener).await.unwrap();
    assert_eq!(endpoint, Endpoint::from_tcp_addr(expected_addr));
    let event = async_rt::task::timeout(Duration::from_secs(2), monitor.next())
        .await
        .expect("timeout waiting for listening monitor event")
        .expect("monitor closed before listening event");
    assert!(matches!(event, SocketEvent::Listening(bound) if bound == endpoint));

    let mut dealer = DealerSocket::new();
    dealer.connect(&endpoint.to_string()).await.unwrap();
    dealer.send(ZmqMessage::from("hello")).await.unwrap();

    let message = async_rt::task::timeout(Duration::from_secs(2), router.recv())
        .await
        .expect("timeout waiting for inherited TCP listener")
        .unwrap();
    assert_eq!(message.get(message.len() - 1).unwrap().as_ref(), b"hello");
    let event = async_rt::task::timeout(Duration::from_secs(2), monitor.next())
        .await
        .expect("timeout waiting for accepted monitor event")
        .expect("monitor closed before accepted event");
    assert!(matches!(event, SocketEvent::Accepted(_, _)));

    assert!(router.close().await.is_empty());
    assert!(dealer.close().await.is_empty());
}

#[async_rt::test]
async fn accepted_peer_that_never_sends_a_greeting_times_out() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handshake_timeout = Duration::from_millis(50);
    let mut options = SocketOptions::default();
    options.handshake_timeout(handshake_timeout);

    let mut router = RouterSocket::with_options(options);
    let mut monitor = router.monitor();
    router.bind_listener(listener).await.unwrap();
    let mut client = async_io::Async::new(std::net::TcpStream::connect(address).unwrap()).unwrap();

    let mut greeting = [0; 64];
    client.read_exact(&mut greeting).await.unwrap();
    let event = async_rt::task::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(SocketEvent::AcceptFailed(error)) = monitor.next().await {
                break error;
            }
        }
    })
    .await
    .expect("timeout waiting for failed handshake event");
    assert!(
        matches!(event, ZmqError::HandshakeTimeout(timeout) if timeout == handshake_timeout),
        "{event:?}"
    );

    let mut byte = [0];
    let read = async_rt::task::timeout(Duration::from_secs(2), client.read(&mut byte))
        .await
        .expect("server did not close the timed-out connection")
        .unwrap();
    assert_eq!(read, 0);
    assert!(router.close().await.is_empty());
}

#[cfg(target_family = "unix")]
#[async_rt::test]
async fn adopts_ipc_listener_and_owns_socket_path() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("zmq-bind-{}-{nonce}.sock", std::process::id()));
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();

    let mut router = RouterSocket::new();
    let endpoint = router.bind_listener(listener).await.unwrap();
    assert_eq!(endpoint, Endpoint::Ipc(Some(path.clone())));

    let mut dealer = DealerSocket::new();
    dealer.connect(&endpoint.to_string()).await.unwrap();
    dealer.send(ZmqMessage::from("hello")).await.unwrap();

    let message = async_rt::task::timeout(Duration::from_secs(2), router.recv())
        .await
        .expect("timeout waiting for inherited IPC listener")
        .unwrap();
    assert_eq!(message.get(message.len() - 1).unwrap().as_ref(), b"hello");

    assert!(router.close().await.is_empty());
    assert!(dealer.close().await.is_empty());
    assert!(!path.exists());
}

#[async_rt::test]
async fn regular_bind_preserves_domain_name() {
    let mut router = RouterSocket::new();
    let endpoint = router.bind("tcp://localhost:0").await.unwrap();

    assert!(matches!(
        endpoint,
        Endpoint::Tcp(Host::Domain(ref host), port) if host == "localhost" && port != 0
    ));
    assert!(router.close().await.is_empty());
}
