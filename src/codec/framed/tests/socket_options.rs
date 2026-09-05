use super::*;
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{
    Endpoint, MultiPeerBackend, Socket, SocketBackend, SocketEvent, SocketOptions, SocketType,
};
use async_trait::async_trait;
use futures::channel::mpsc;
use futures::SinkExt;
use parking_lot::Mutex as ParkingMutex;
use std::collections::HashMap;
use std::time::Duration;

// Capture the actual framed connection after the normal socket handshake so
// the tests detect configuration lost anywhere between SocketOptions and I/O.
struct RecordingBackend {
    options: SocketOptions,
    peers: mpsc::UnboundedSender<(PeerIdentity, FramedIo)>,
    monitor: ParkingMutex<Option<mpsc::Sender<SocketEvent>>>,
}

impl SocketBackend for RecordingBackend {
    fn socket_type(&self) -> SocketType {
        SocketType::PAIR
    }
    fn socket_options(&self) -> &SocketOptions {
        &self.options
    }
    fn shutdown(&self) {}
    fn monitor(&self) -> &ParkingMutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.monitor
    }
}

#[async_trait]
impl MultiPeerBackend for RecordingBackend {
    async fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: FramedIo) {
        assert!(self.peers.unbounded_send((peer_id.clone(), io)).is_ok());
    }
    fn peer_disconnected(&self, _peer_id: &PeerIdentity) {}
}

struct RecordingSocket {
    backend: Arc<RecordingBackend>,
    peers: mpsc::UnboundedReceiver<(PeerIdentity, FramedIo)>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

#[async_trait]
impl Socket for RecordingSocket {
    fn with_options(options: SocketOptions) -> Self {
        let (sender, peers) = mpsc::unbounded();
        Self {
            backend: Arc::new(RecordingBackend {
                options,
                peers: sender,
                monitor: ParkingMutex::new(None),
            }),
            peers,
            binds: HashMap::new(),
        }
    }
    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }
    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle> {
        &mut self.binds
    }
    fn monitor(&mut self) -> mpsc::Receiver<SocketEvent> {
        let (sender, receiver) = mpsc::channel(16);
        *self.backend.monitor.lock() = Some(sender);
        receiver
    }
}

impl RecordingSocket {
    async fn connected_peer(&mut self) -> (PeerIdentity, FramedIo) {
        async_rt::task::timeout(Duration::from_secs(5), self.peers.next())
            .await
            .expect("peer handshake timed out")
            .expect("peer channel closed")
    }
}

fn configured_socket() -> RecordingSocket {
    let mut options = SocketOptions::default();
    options.read_buffer_recovery(true);
    RecordingSocket::with_options(options)
}

async fn exchange(left: &mut RecordingSocket, right: &mut RecordingSocket, endpoint: Endpoint) {
    right.connect(&endpoint.to_string()).await.unwrap();
    let (_, mut left_io) = left.connected_peer().await;
    let (_, mut right_io) = right.connected_peer().await;
    assert_eq!(
        left_io.read_half.short_read_seen.is_some(),
        left.backend.options.read_buffer_recovery
    );
    assert_eq!(
        right_io.read_half.short_read_seen.is_some(),
        right.backend.options.read_buffer_recovery
    );
    let mut message = crate::ZmqMessage::from(Bytes::from(vec![1; 338_729]));
    message.push_back(Bytes::from_static(b"last part"));
    // IPC can backpressure a large write before the whole message is sent.
    let received = async_rt::task::timeout(Duration::from_secs(5), async {
        let (sent, received) = futures::join!(
            right_io.write_half.send(Message::Message(message.clone())),
            left_io.read_half.next(),
        );
        sent.unwrap();
        received.unwrap().unwrap()
    })
    .await
    .expect("multipart exchange timed out");
    let Message::Message(received) = received else {
        panic!("expected multipart message")
    };
    assert!(received.iter().eq(message.iter()));
}

#[cfg(feature = "tcp-transport")]
#[async_rt::test]
async fn read_buffer_tcp_connect_and_bind_use_independent_socket_options() {
    let mut left = configured_socket();
    let mut right = RecordingSocket::new();
    let endpoint = left.bind("tcp://127.0.0.1:0").await.unwrap();
    exchange(&mut left, &mut right, endpoint).await;
    assert!(left.close().await.is_empty());
    assert!(right.close().await.is_empty());
}

#[cfg(all(feature = "ipc-transport", target_family = "unix"))]
#[async_rt::test]
async fn read_buffer_adopted_ipc_listener_uses_socket_options() {
    let path = std::env::temp_dir().join(format!("z-rb-{}.sock", uuid::Uuid::new_v4()));
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let mut left = configured_socket();
    let mut right = RecordingSocket::new();
    let endpoint = left.bind_listener(listener).await.unwrap();
    exchange(&mut left, &mut right, endpoint).await;
    assert!(left.close().await.is_empty());
    assert!(right.close().await.is_empty());
    assert!(!path.exists());
}

#[cfg(feature = "tcp-transport")]
#[async_rt::test]
async fn read_buffer_reconnect_preserves_socket_options() {
    let mut left = RecordingSocket::new();
    let mut right = configured_socket();
    let endpoint = left.bind("tcp://127.0.0.1:0").await.unwrap();
    right.connect(&endpoint.to_string()).await.unwrap();
    let (peer_id, first_io) = right.connected_peer().await;
    drop(first_io);
    drop(left.connected_peer().await);

    let (registrations, mut registered) = mpsc::unbounded();
    let handle = crate::reconnect::spawn_reconnect_task(
        endpoint,
        right.backend(),
        peer_id,
        Box::new(move |id, sender| {
            assert!(registrations.unbounded_send((id, sender)).is_ok());
        }),
        crate::reconnect::ReconnectConfig::default(),
    );
    let (peer_id, mut notifier) = registered.next().await.unwrap();
    notifier.send(peer_id).await.unwrap();
    let (_, reconnected_io) = right.connected_peer().await;
    handle.shutdown();
    assert_eq!(
        reconnected_io.read_half.short_read_seen.is_some(),
        right.backend.options.read_buffer_recovery
    );
    drop(left.connected_peer().await);
    assert!(left.close().await.is_empty());
    assert!(right.close().await.is_empty());
}
