//! Regression test for `disconnect()` racing an in-flight reconnection attempt.
//!
//! `ReconnectHandle::shutdown()` is only observed by the reconnect task at its
//! `select!` points. A task sitting inside the connect/handshake sequence used
//! to run that attempt to completion and register the resulting peer -- for an
//! endpoint the caller had already disconnected, and which `disconnect()` could
//! no longer reach because it had removed its `connects` entry.

use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::{Endpoint, SocketEvent};

use futures::{FutureExt, StreamExt};
use std::io::Read;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

/// What a stub listener reports back to the test.
enum Stub {
    /// A reconnect attempt connected to us.
    Accepted,
    /// That connection was closed by the other side.
    Closed,
}

/// A listener that completes the TCP accept but never sends a ZMTP greeting,
/// parking any reconnect attempt inside the handshake for as long as we like.
fn spawn_black_hole_listener(port: u16) -> std_mpsc::Receiver<Stub> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("failed to squat port");
    let (tx, rx) = std_mpsc::channel();

    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = tx.send(Stub::Accepted);

        // Never greet. Just wait for the peer to give up and close, which is
        // what an abandoned attempt looks like from this side.
        let mut buf = [0u8; 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {} // their greeting; deliberately unanswered
            }
        }
        let _ = tx.send(Stub::Closed);
    });

    rx
}

fn port_of(endpoint: &Endpoint) -> u16 {
    match endpoint {
        Endpoint::Tcp(_, port) => *port,
        other => unreachable!("test binds over tcp, got {other:?}"),
    }
}

/// Waits until traffic actually flows between the pair.
///
/// A `Connected` event on the SUB only means *its* half of the handshake
/// finished; the PUB may not have registered the subscriber yet. Closing the
/// PUB during that window leaves the connection dangling, so the SUB never
/// observes EOF and no reconnection is ever armed.
async fn establish(publisher: &mut zeromq::PubSocket, subscriber: &mut zeromq::SubSocket) {
    for _ in 0..50 {
        publisher
            .send(zeromq::ZmqMessage::from("ping"))
            .await
            .expect("send failed");
        if async_rt::task::timeout(Duration::from_millis(100), subscriber.recv())
            .await
            .is_ok()
        {
            return;
        }
    }
    panic!("publisher and subscriber never exchanged traffic");
}

/// Awaits a stub report without blocking the executor.
///
/// The reconnect task shares this thread under a current-thread runtime, so a
/// blocking `recv_timeout` here would stall the very task we are waiting on.
async fn await_stub(signal: &std_mpsc::Receiver<Stub>, budget: Duration) -> Option<Stub> {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if let Ok(event) = signal.try_recv() {
            return Some(event);
        }
        async_rt::task::sleep(Duration::from_millis(20)).await;
    }
    signal.try_recv().ok()
}

/// Drives `recv()` for a fixed duration so the SUB notices peer loss.
async fn pump_for(subscriber: &mut zeromq::SubSocket, budget: Duration) {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        futures::select! {
            _ = subscriber.recv().fuse() => {}
            _ = async_rt::task::sleep(Duration::from_millis(20)).fuse() => {}
        }
    }
}

/// Drives `recv()` so the SUB notices peer loss, which is what arms the
/// reconnect task. Returns once `signal` fires or `budget` elapses.
async fn pump_until(
    subscriber: &mut zeromq::SubSocket,
    signal: &std_mpsc::Receiver<Stub>,
    budget: Duration,
) -> Option<Stub> {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if let Ok(event) = signal.try_recv() {
            return Some(event);
        }
        // recv() is cancel-safe, so racing it against a tick is fine.
        futures::select! {
            _ = subscriber.recv().fuse() => {}
            _ = async_rt::task::sleep(Duration::from_millis(20)).fuse() => {}
        }
    }
    signal.try_recv().ok()
}

#[async_rt::test]
async fn disconnect_cancels_an_in_flight_reconnect_attempt() {
    pretty_env_logger::try_init().ok();

    let mut publisher = zeromq::PubSocket::new();
    let endpoint = publisher
        .bind("tcp://127.0.0.1:0")
        .await
        .expect("bind failed");
    let port = port_of(&endpoint);

    let mut subscriber = zeromq::SubSocket::new();
    let mut monitor = subscriber.monitor();
    subscriber.subscribe("").await.expect("subscribe failed");
    subscriber
        .connect(&endpoint.to_string())
        .await
        .expect("connect failed");

    let connected = async_rt::task::timeout(Duration::from_secs(2), monitor.next())
        .await
        .expect("timed out waiting for Connected")
        .expect("monitor closed");
    assert!(
        matches!(connected, SocketEvent::Connected(..)),
        "{connected:?}"
    );
    establish(&mut publisher, &mut subscriber).await;

    // Drop the publisher and squat its port with a listener that never greets,
    // so the reconnect task parks inside the handshake instead of failing fast.
    publisher.close().await;
    async_rt::task::sleep(Duration::from_millis(50)).await;
    let stub = spawn_black_hole_listener(port);

    // Pump recv() until the reconnect attempt actually reaches the stub.
    let accepted = pump_until(&mut subscriber, &stub, Duration::from_secs(5)).await;
    assert!(
        matches!(accepted, Some(Stub::Accepted)),
        "reconnect task never attempted a reconnection; the race cannot be exercised"
    );

    // The attempt is now parked in greet_exchange. Pull the rug out.
    subscriber
        .disconnect(endpoint)
        .await
        .expect("disconnect failed");

    // The abandoned attempt must close its socket. The connect timeout is 30s,
    // so anything this prompt is shutdown doing the work, not the timeout.
    let closed = await_stub(&stub, Duration::from_secs(3))
        .await
        .expect("in-flight reconnect attempt was not abandoned after disconnect()");
    assert!(matches!(closed, Stub::Closed));

    // And no peer may be registered after the disconnect.
    let resurrected = async_rt::task::timeout(Duration::from_secs(2), async {
        while let Some(event) = monitor.next().await {
            if matches!(event, SocketEvent::Connected(..)) {
                return true;
            }
        }
        false
    })
    .await;

    assert!(
        resurrected.is_err() || !resurrected.expect("checked"),
        "reconnect task registered a peer for an endpoint that was disconnected"
    );
}

/// General property check: a socket must not be connected after `disconnect()`,
/// even when a reconnection was already armed and the endpoint is reachable
/// again.
///
/// Note this does *not* discriminate the `shutdown_flag` half of the fix -- it
/// passes against the unfixed code too, because hitting that window requires
/// `peer_connected` to complete in the gap between `shutdown()` and
/// `disconnect()`'s peer scan, which cannot be forced from outside the crate.
/// It is kept as a guard on the broader invariant.
#[async_rt::test]
async fn disconnect_is_final_even_if_a_reconnect_lands_late() {
    let mut publisher = zeromq::PubSocket::new();
    let endpoint = publisher
        .bind("tcp://127.0.0.1:0")
        .await
        .expect("bind failed");
    let port = port_of(&endpoint);

    let mut subscriber = zeromq::SubSocket::new();
    let mut monitor = subscriber.monitor();
    subscriber.subscribe("").await.expect("subscribe failed");
    subscriber
        .connect(&endpoint.to_string())
        .await
        .expect("connect failed");
    let _ = monitor.next().await;
    establish(&mut publisher, &mut subscriber).await;

    publisher.close().await;
    async_rt::task::sleep(Duration::from_millis(50)).await;

    // A real publisher this time, so the reconnect can genuinely succeed and
    // race the disconnect rather than parking forever.
    let mut replacement = zeromq::PubSocket::new();
    replacement
        .bind(&format!("tcp://127.0.0.1:{port}"))
        .await
        .expect("rebind failed");

    // Arm the reconnect, then disconnect while it is in flight.
    pump_for(&mut subscriber, Duration::from_millis(150)).await;

    subscriber
        .disconnect(endpoint)
        .await
        .expect("disconnect failed");

    // Give any in-flight attempt time to complete and self-clean.
    async_rt::task::sleep(Duration::from_secs(1)).await;

    // Drain the monitor and require that the last connection-lifecycle event is
    // a disconnect: the socket must not have come back up.
    let mut last = None;
    while let Ok(Some(event)) =
        async_rt::task::timeout(Duration::from_millis(100), monitor.next()).await
    {
        match event {
            SocketEvent::Connected(..) | SocketEvent::Disconnected(_) => last = Some(event),
            _ => {}
        }
    }

    assert!(
        matches!(last, Some(SocketEvent::Disconnected(_)) | None),
        "socket ended up connected after an explicit disconnect: {last:?}"
    );
}
