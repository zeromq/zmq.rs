use std::convert::TryInto;
use std::panic;
use std::thread;
use std::time::Duration;

use futures::channel::oneshot;
use zeromq::__async_rt as async_rt;

/// NOTE: This will block. Careful when using in async code.
#[allow(dead_code)]
pub fn get_monitor_event(monitor: &zmq2::Socket) -> (zmq2::SocketEvent, u32, String) {
    assert_eq!(monitor.get_socket_type().unwrap(), zmq2::PAIR);
    let mut msgs = monitor.recv_multipart(0).expect("Monitor couldn't recv");
    assert_eq!(msgs.len(), 2);

    assert_eq!(msgs[0].len(), 6);
    let event: [u8; 2] = msgs[0][..2].try_into().unwrap();
    let event_value: [u8; 4] = msgs[0][2..].try_into().unwrap();
    // TODO: what is the endianness of zmq here? Is it platform dependent?
    let event = zmq2::SocketEvent::from_raw(u16::from_le_bytes(event));
    let event_value = u32::from_le_bytes(event_value);
    let remote_endpoint = String::from_utf8(msgs.pop().unwrap()).unwrap();

    (event, event_value, remote_endpoint)
}

/// Configures `their_sock` with a socket monitor, and returns the monitor
#[allow(dead_code)]
pub fn setup_monitor(
    ctx: &zmq2::Context,
    their_sock: &zmq2::Socket,
    monitor_endpoint: &str,
) -> zmq2::Socket {
    their_sock
        .monitor(monitor_endpoint, zmq2::SocketEvent::ALL.to_raw().into())
        .expect("Failed to set up monitor");
    let their_monitor = ctx.socket(zmq2::PAIR).expect("Couldnt make pair socket");
    their_monitor
        .connect(monitor_endpoint)
        .expect("Failed to connect monitor");
    their_monitor
}

#[allow(dead_code)]
pub async fn join_thread<T>(
    handle: thread::JoinHandle<T>,
    timeout: Duration,
    label: &'static str,
) -> T
where
    T: Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    thread::spawn(move || {
        let _ = sender.send(handle.join());
    });

    let joined = async_rt::task::timeout(timeout, receiver)
        .await
        .unwrap_or_else(|_| panic!("{label}: thread join timed out"))
        .unwrap_or_else(|_| panic!("{label}: join relay dropped"));

    match joined {
        Ok(value) => value,
        Err(payload) => panic::resume_unwind(payload),
    }
}

#[allow(dead_code)]
pub async fn recv_string_on_thread(
    socket: zmq2::Socket,
    timeout: Duration,
    label: &'static str,
) -> (zmq2::Socket, String) {
    let handle = thread::spawn(move || {
        let received = socket.recv_string(0);
        (socket, received)
    });
    let (socket, received) = join_thread(handle, timeout, label).await;
    let message = received
        .unwrap_or_else(|error| panic!("{label}: failed to recv: {error}"))
        .unwrap_or_else(|bytes| panic!("{label}: invalid UTF-8: {bytes:?}"));

    (socket, message)
}
