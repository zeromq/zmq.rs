//! Peer I/O after transport connect/accept: framed ZMTP or inproc message channels.

use crate::async_rt;
use crate::codec::{CodecError, CodecResult, FramedIo, Message, ZmqFramedRead, ZmqFramedWrite};
use crate::write_queue::write_message_queue;

use futures::channel::mpsc;
use futures::{SinkExt, Stream};

use std::pin::Pin;
use std::task::{Context, Poll};

const PEER_SEND_QUEUE_CAPACITY: usize = 100_000;

/// Connected peer transport handed to socket backends.
pub enum PeerIo {
    /// Byte stream with ZMTP framing (TCP/IPC).
    Framed(FramedIo),
    /// Direct message channels (`inproc://`); no ZMTP serialization.
    Inproc {
        /// Messages this socket sends to the peer.
        outbound: mpsc::Sender<Message>,
        /// Messages this socket receives from the peer.
        inbound: mpsc::Receiver<Message>,
    },
}

/// Receive half used by fair queues and REQ peers.
pub enum PeerRecv {
    Framed(ZmqFramedRead),
    Inproc(mpsc::Receiver<Message>),
}

impl Stream for PeerRecv {
    type Item = CodecResult<Message>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            Self::Framed(read) => Pin::new(read).poll_next(cx),
            Self::Inproc(read) => match Pin::new(read).poll_next(cx) {
                Poll::Ready(Some(message)) => Poll::Ready(Some(Ok(message))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

/// Send half that either writes ZMTP frames or pushes into an inproc channel.
pub enum PeerSend {
    Framed(ZmqFramedWrite),
    Channel(mpsc::Sender<Message>),
}

impl PeerSend {
    pub async fn send(&mut self, message: Message) -> Result<(), PeerSendError> {
        match self {
            Self::Framed(write) => write.send(message).await.map_err(PeerSendError::Codec),
            Self::Channel(send) => send.send(message).await.map_err(PeerSendError::Channel),
        }
    }
}

#[derive(Debug)]
pub enum PeerSendError {
    Codec(CodecError),
    Channel(mpsc::SendError),
}

impl From<PeerSendError> for crate::ZmqError {
    fn from(error: PeerSendError) -> Self {
        match error {
            PeerSendError::Codec(error) => error.into(),
            PeerSendError::Channel(error) => error.into(),
        }
    }
}

impl PeerSendError {
    pub fn is_disconnected(&self) -> bool {
        match self {
            Self::Codec(CodecError::Io(error)) => {
                error.kind() == std::io::ErrorKind::BrokenPipe
                    || error.kind() == std::io::ErrorKind::ConnectionReset
            }
            Self::Channel(error) => error.is_disconnected(),
            _ => false,
        }
    }
}

/// Splits [`PeerIo`] into send/recv halves without an intermediate writer task.
///
/// Prefer this for SUB/XPUB control traffic that must reach the wire before the
/// calling task proceeds (important when mixed with blocking libzmq calls).
pub(crate) fn split_peer_io(io: PeerIo) -> (PeerSend, PeerRecv) {
    match io {
        PeerIo::Inproc { outbound, inbound } => {
            (PeerSend::Channel(outbound), PeerRecv::Inproc(inbound))
        }
        PeerIo::Framed(io) => {
            let (recv_queue, send_queue) = io.into_parts();
            (PeerSend::Framed(send_queue), PeerRecv::Framed(recv_queue))
        }
    }
}

/// Opens [`PeerIo`] into a send queue and receive stream.
///
/// For [`PeerIo::Framed`], spawns a writer that encodes messages onto the
/// framed write half. For [`PeerIo::Inproc`], the outbound sender is the peer's
/// inbound channel directly (no framing task).
pub(crate) fn install_peer_io(
    io: PeerIo,
    on_write_fail: impl FnOnce() + Send + 'static,
) -> (mpsc::Sender<Message>, PeerRecv) {
    match io {
        PeerIo::Inproc { outbound, inbound } => (outbound, PeerRecv::Inproc(inbound)),
        PeerIo::Framed(io) => {
            let (recv_queue, send_queue) = io.into_parts();
            let (queue_sender, queue_receiver) = mpsc::channel(PEER_SEND_QUEUE_CAPACITY);
            async_rt::task::spawn(async move {
                if write_message_queue(queue_receiver, send_queue)
                    .await
                    .is_err()
                    {
                        on_write_fail();
                    }
            });
            (queue_sender, PeerRecv::Framed(recv_queue))
        }
    }
}

/// Creates a cross-linked inproc peer pair (bind end, connect end).
pub(crate) fn inproc_peer_pair() -> (PeerIo, PeerIo) {
    let (to_connect_tx, to_connect_rx) = mpsc::channel(PEER_SEND_QUEUE_CAPACITY);
    let (to_bind_tx, to_bind_rx) = mpsc::channel(PEER_SEND_QUEUE_CAPACITY);

    let bind_end = PeerIo::Inproc {
        outbound: to_connect_tx,
        inbound: to_bind_rx,
    };
    let connect_end = PeerIo::Inproc {
        outbound: to_bind_tx,
        inbound: to_connect_rx,
    };
    (bind_end, connect_end)
}
