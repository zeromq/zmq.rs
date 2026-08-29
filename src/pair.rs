use crate::codec::Message;
use crate::fair_queue::{FairQueue, QueueInner};
use crate::peer_io::{install_peer_io, PeerIo, PeerRecv};
use crate::transport::AcceptStopHandle;
use crate::util::PeerIdentity;
use crate::{
    Endpoint, MultiPeerBackend, Socket, SocketBackend, SocketEvent, SocketOptions, SocketRecv,
    SocketSend, SocketType, ZmqError, ZmqMessage, ZmqResult,
};

use async_trait::async_trait;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::sync::Arc;

struct PairPeer {
    peer_id: PeerIdentity,
    send_queue: mpsc::Sender<Message>,
}

/// Backend for [`PairSocket`]: admits at most one peer.
///
/// Peer admission is atomic under a mutex so concurrent handshakes cannot both
/// install a peer (unlike checking `peers.is_empty()` then awaiting upsert).
pub(crate) struct PairBackend {
    peer: Mutex<Option<PairPeer>>,
    fair_queue_inner: Arc<Mutex<QueueInner<PeerRecv, PeerIdentity>>>,
    socket_options: SocketOptions,
    socket_monitor: Mutex<Option<mpsc::Sender<SocketEvent>>>,
}

impl PairBackend {
    fn new(
        fair_queue_inner: Arc<Mutex<QueueInner<PeerRecv, PeerIdentity>>>,
        options: SocketOptions,
    ) -> Self {
        Self {
            peer: Mutex::new(None),
            fair_queue_inner,
            socket_options: options,
            socket_monitor: Mutex::new(None),
        }
    }

    async fn send_message(&self, message: Message) -> ZmqResult<()> {
        let mut send_queue = {
            let peer = self.peer.lock();
            match &*peer {
                Some(peer) => peer.send_queue.clone(),
                None => {
                    return match message {
                        Message::Greeting(_) => {
                            Err(ZmqError::Socket("Sending greeting is not supported"))
                        }
                        Message::Command(_) => {
                            Err(ZmqError::Socket("Sending commands is not supported"))
                        }
                        Message::Message(m) => Err(ZmqError::ReturnToSender {
                            reason: "Not connected to peers. Unable to send messages",
                            message: m,
                        }),
                    };
                }
            }
        };

        match send_queue.try_send(message) {
            Ok(()) => Ok(()),
            Err(error) if error.is_full() => send_queue
                .send(error.into_inner())
                .await
                .map_err(Into::into),
            Err(error) => Err(error.into_send_error().into()),
        }
    }
}

impl SocketBackend for PairBackend {
    fn socket_type(&self) -> SocketType {
        SocketType::PAIR
    }

    fn socket_options(&self) -> &SocketOptions {
        &self.socket_options
    }

    fn shutdown(&self) {
        self.peer.lock().take();
        self.fair_queue_inner.lock().clear();
    }

    fn monitor(&self) -> &Mutex<Option<mpsc::Sender<SocketEvent>>> {
        &self.socket_monitor
    }
}

#[async_trait]
impl MultiPeerBackend for PairBackend {
    async fn peer_connected(self: Arc<Self>, peer_id: &PeerIdentity, io: PeerIo) {
        let recv_queue = {
            let mut slot = self.peer.lock();
            if slot.is_some() {
                log::debug!("PAIR socket rejecting additional peer {peer_id:?}");
                drop(io);
                return;
            }

            let backend = self.clone();
            let writer_peer_id = peer_id.clone();
            let (queue_sender, recv_queue) = install_peer_io(io, move || {
                backend.peer_disconnected(&writer_peer_id);
            });

            *slot = Some(PairPeer {
                peer_id: peer_id.clone(),
                send_queue: queue_sender,
            });
            recv_queue
        };

        self.fair_queue_inner
            .lock()
            .insert(peer_id.clone(), recv_queue);
    }

    fn peer_disconnected(&self, peer_id: &PeerIdentity) {
        {
            let mut slot = self.peer.lock();
            if slot.as_ref().is_some_and(|p| p.peer_id == *peer_id) {
                slot.take();
            }
        }
        self.fair_queue_inner.lock().remove(peer_id);
    }
}

/// Exclusive pair socket (`ZMQ_PAIR`).
///
/// A `PAIR` socket is connected to exactly one peer. Further connection attempts
/// are rejected (the extra peer is dropped), matching libzmq behaviour.
pub struct PairSocket {
    backend: Arc<PairBackend>,
    fair_queue: FairQueue<PeerRecv, PeerIdentity>,
    binds: HashMap<Endpoint, AcceptStopHandle>,
}

impl Drop for PairSocket {
    fn drop(&mut self) {
        self.backend.shutdown();
    }
}

#[async_trait]
impl Socket for PairSocket {
    fn with_options(options: SocketOptions) -> Self {
        let mut fair_queue = FairQueue::new(true);
        let backend = Arc::new(PairBackend::new(fair_queue.inner(), options));

        let backend_weak = Arc::downgrade(&backend);
        fair_queue.set_on_disconnect(move |peer_id: PeerIdentity| {
            if let Some(backend) = backend_weak.upgrade() {
                backend.peer_disconnected(&peer_id);
            }
        });

        Self {
            backend,
            fair_queue,
            binds: HashMap::new(),
        }
    }

    fn backend(&self) -> Arc<dyn MultiPeerBackend> {
        self.backend.clone()
    }

    fn binds(&mut self) -> &mut HashMap<Endpoint, AcceptStopHandle, RandomState> {
        &mut self.binds
    }

    fn monitor(&mut self) -> mpsc::Receiver<SocketEvent> {
        let (sender, receiver) = mpsc::channel(1024);
        self.backend.socket_monitor.lock().replace(sender);
        receiver
    }
}

#[async_trait]
impl SocketRecv for PairSocket {
    async fn recv(&mut self) -> ZmqResult<ZmqMessage> {
        loop {
            match self.fair_queue.next().await {
                Some((_peer_id, Ok(Message::Message(message)))) => {
                    return Ok(message);
                }
                Some((_peer_id, Ok(_))) => {
                    // Ignore non-message frames
                }
                Some((peer_id, Err(e))) => {
                    self.backend.peer_disconnected(&peer_id);
                    return Err(e.into());
                }
                None => {
                    return Err(ZmqError::NoMessage);
                }
            }
        }
    }
}

#[async_trait]
impl SocketSend for PairSocket {
    async fn send(&mut self, message: ZmqMessage) -> ZmqResult<()> {
        self.backend.send_message(Message::Message(message)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::FramedIo;
    use crate::util::PeerIdentity;

    use futures::{AsyncRead, AsyncWrite};
    use std::io;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};

    struct NullIo;

    impl AsyncRead for NullIo {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut [u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }
    }

    impl AsyncWrite for NullIo {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn framed_null() -> PeerIo {
        PeerIo::Framed(FramedIo::new(Box::new(NullIo), Box::new(NullIo)))
    }

    #[crate::async_rt::test]
    async fn pair_backend_admits_only_one_peer_under_contention() {
        let fair_queue = FairQueue::new(true);
        let backend = Arc::new(PairBackend::new(
            fair_queue.inner(),
            SocketOptions::default(),
        ));

        let admitted = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..32 {
            let backend = backend.clone();
            let admitted = admitted.clone();
            tasks.push(crate::async_rt::task::spawn(async move {
                let peer_id = PeerIdentity::new();
                backend
                    .clone()
                    .peer_connected(&peer_id, framed_null())
                    .await;
                if backend
                    .peer
                    .lock()
                    .as_ref()
                    .is_some_and(|p| p.peer_id == peer_id)
                {
                    admitted.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }

        for task in tasks {
            let _ = task.await;
        }

        assert_eq!(admitted.load(Ordering::SeqCst), 1);
        assert!(backend.peer.lock().is_some());
    }
}
