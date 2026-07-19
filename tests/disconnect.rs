use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::{SocketEvent, ZmqError, ZmqMessage};

use futures::channel::mpsc;
use futures::StreamExt;
use std::time::Duration;

const LOCAL: &str = "tcp://127.0.0.1:0";

async fn next_event(monitor: &mut mpsc::Receiver<SocketEvent>) -> SocketEvent {
    async_rt::task::timeout(Duration::from_secs(2), monitor.next())
        .await
        .expect("timed out waiting for a socket event")
        .expect("monitor channel closed")
}

/// Asserts that the connecting side of a `binder`/`connector` pair can be
/// disconnected by the endpoint it was given, for every socket type.
macro_rules! disconnect_teardown_test {
    ($name:ident, $binder:ty, $connector:ty) => {
        #[async_rt::test]
        async fn $name() {
            let mut binder = <$binder>::new();
            let endpoint = binder.bind(LOCAL).await.expect("bind failed");

            let mut connector = <$connector>::new();
            let mut monitor = connector.monitor();
            connector
                .connect(&endpoint.to_string())
                .await
                .expect("connect failed");

            let event = next_event(&mut monitor).await;
            assert!(matches!(event, SocketEvent::Connected(..)), "{event:?}");

            connector
                .disconnect(endpoint)
                .await
                .expect("disconnect failed");

            let event = next_event(&mut monitor).await;
            assert!(matches!(event, SocketEvent::Disconnected(_)), "{event:?}");
        }
    };
}

disconnect_teardown_test!(disconnect_req, zeromq::RepSocket, zeromq::ReqSocket);
disconnect_teardown_test!(disconnect_rep, zeromq::DealerSocket, zeromq::RepSocket);
disconnect_teardown_test!(
    disconnect_dealer,
    zeromq::RouterSocket,
    zeromq::DealerSocket
);
disconnect_teardown_test!(
    disconnect_router,
    zeromq::DealerSocket,
    zeromq::RouterSocket
);
disconnect_teardown_test!(disconnect_push, zeromq::PullSocket, zeromq::PushSocket);
disconnect_teardown_test!(disconnect_pull, zeromq::PushSocket, zeromq::PullSocket);
disconnect_teardown_test!(disconnect_pub, zeromq::SubSocket, zeromq::PubSocket);
disconnect_teardown_test!(disconnect_sub, zeromq::PubSocket, zeromq::SubSocket);
disconnect_teardown_test!(disconnect_xpub, zeromq::XSubSocket, zeromq::XPubSocket);
disconnect_teardown_test!(disconnect_xsub, zeromq::XPubSocket, zeromq::XSubSocket);

#[async_rt::test]
async fn disconnect_unconnected_endpoint_gives_no_such_connection() {
    let mut socket = zeromq::DealerSocket::new();

    let err = socket
        .disconnect("tcp://127.0.0.1:12345")
        .await
        .expect_err("disconnect of an unconnected endpoint should fail");

    assert!(matches!(err, ZmqError::NoSuchConnection(_)), "{err:?}");
}

/// A bound endpoint is not a connected one, so it is not disconnectable.
#[async_rt::test]
async fn disconnect_does_not_match_bound_endpoints() {
    let mut socket = zeromq::RouterSocket::new();
    let endpoint = socket.bind(LOCAL).await.expect("bind failed");

    let err = socket
        .disconnect(endpoint)
        .await
        .expect_err("a bound endpoint should not be disconnectable");

    assert!(matches!(err, ZmqError::NoSuchConnection(_)), "{err:?}");
}

/// Disconnecting one endpoint must leave the socket's other connections intact.
/// This is what proves `peer_list_by_endpoint` filters rather than just counts.
#[async_rt::test]
async fn disconnect_only_affects_the_named_endpoint() {
    let mut kept = zeromq::PullSocket::new();
    let kept_endpoint = kept.bind(LOCAL).await.expect("bind failed");

    let mut dropped = zeromq::PullSocket::new();
    let dropped_endpoint = dropped.bind(LOCAL).await.expect("bind failed");

    let mut push = zeromq::PushSocket::new();
    push.connect(&kept_endpoint.to_string()).await.unwrap();
    push.connect(&dropped_endpoint.to_string()).await.unwrap();

    push.disconnect(dropped_endpoint)
        .await
        .expect("disconnect failed");

    // With only one peer left, every message must land on `kept`.
    for i in 0..4 {
        push.send(ZmqMessage::from(format!("m{i}"))).await.unwrap();
    }
    for i in 0..4 {
        let message = async_rt::task::timeout(Duration::from_secs(2), kept.recv())
            .await
            .expect("timed out waiting for a message on the kept endpoint")
            .unwrap();
        assert_eq!(message.get(0).unwrap().as_ref(), format!("m{i}").as_bytes());
    }
}

/// Regression test: `peer_disconnected` notifies the reconnect task, so an
/// explicit `disconnect()` must stop that task first or the socket immediately
/// reconnects to the endpoint it was just disconnected from.
#[async_rt::test]
async fn disconnect_does_not_trigger_reconnection() {
    let mut publisher = zeromq::PubSocket::new();
    let endpoint = publisher.bind(LOCAL).await.expect("bind failed");

    // SUB connects through the reconnecting path (`connect_with_reconnect`).
    let mut subscriber = zeromq::SubSocket::new();
    let mut monitor = subscriber.monitor();
    subscriber.subscribe("").await.unwrap();
    subscriber
        .connect(&endpoint.to_string())
        .await
        .expect("connect failed");

    let event = next_event(&mut monitor).await;
    assert!(matches!(event, SocketEvent::Connected(..)), "{event:?}");

    subscriber
        .disconnect(endpoint)
        .await
        .expect("disconnect failed");

    let event = next_event(&mut monitor).await;
    assert!(matches!(event, SocketEvent::Disconnected(_)), "{event:?}");

    // The reconnect task backs off from 100ms, so a second is many attempts.
    let reconnected = async_rt::task::timeout(Duration::from_secs(1), async {
        while let Some(event) = monitor.next().await {
            if matches!(event, SocketEvent::Connected(..)) {
                return true;
            }
        }
        false
    })
    .await;

    assert!(
        reconnected.is_err() || !reconnected.unwrap(),
        "socket reconnected after an explicit disconnect"
    );
}

#[async_rt::test]
async fn close_disconnects_every_connected_endpoint() {
    let mut first = zeromq::PullSocket::new();
    let first_endpoint = first.bind(LOCAL).await.expect("bind failed");

    let mut second = zeromq::PullSocket::new();
    let second_endpoint = second.bind(LOCAL).await.expect("bind failed");

    let mut push = zeromq::PushSocket::new();
    let mut monitor = push.monitor();
    push.connect(&first_endpoint.to_string()).await.unwrap();
    push.connect(&second_endpoint.to_string()).await.unwrap();

    assert!(matches!(
        next_event(&mut monitor).await,
        SocketEvent::Connected(..)
    ));
    assert!(matches!(
        next_event(&mut monitor).await,
        SocketEvent::Connected(..)
    ));

    let errs = push.close().await;
    assert!(errs.is_empty(), "close reported errors: {errs:?}");

    // Both connections must have been torn down, not just one.
    assert!(matches!(
        next_event(&mut monitor).await,
        SocketEvent::Disconnected(_)
    ));
    assert!(matches!(
        next_event(&mut monitor).await,
        SocketEvent::Disconnected(_)
    ));
}
