use crate::codec::CodecResult;
use crate::codec::FramedIo;
use crate::fair_queue::FairQueue;
use crate::*;

use bytes::Bytes;
use futures::lock::Mutex;
use futures::stream::StreamExt;
use futures::SinkExt;
use std::convert::{TryFrom, TryInto};
use std::sync::Arc;
use uuid::Uuid;

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Clone)]
pub struct PeerIdentity(Vec<u8>);

impl PeerIdentity {
    pub fn new() -> Self {
        let id = Uuid::new_v4();
        Self(id.as_bytes().to_vec())
    }
}

impl Default for PeerIdentity {
    fn default() -> Self {
        Self::new()
    }
}

impl TryFrom<Vec<u8>> for PeerIdentity {
    type Error = ZmqError;

    fn try_from(data: Vec<u8>) -> Result<Self, ZmqError> {
        if data.is_empty() {
            Ok(PeerIdentity::new())
        } else if data.len() > 255 {
            Err(ZmqError::Other(
                "ZMQ_IDENTITY should not be more than 255 bytes long",
            ))
        } else {
            Ok(Self(data))
        }
    }
}

impl From<PeerIdentity> for Vec<u8> {
    fn from(p_id: PeerIdentity) -> Self {
        p_id.0
    }
}

impl From<PeerIdentity> for Bytes {
    fn from(p_id: PeerIdentity) -> Self {
        Bytes::from(p_id.0)
    }
}

pub(crate) struct Peer {
    pub(crate) _identity: PeerIdentity,
    pub(crate) send_queue: mpsc::Sender<Message>,
    pub(crate) recv_queue: Arc<Mutex<mpsc::Receiver<Message>>>,
    pub(crate) recv_queue_in: mpsc::Sender<Message>,
    pub(crate) _io_close_handle: futures::channel::oneshot::Sender<bool>,
}

const COMPATIBILITY_MATRIX: [u8; 121] = [
    // PAIR, PUB, SUB, REQ, REP, DEALER, ROUTER, PULL, PUSH, XPUB, XSUB
    1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // PAIR
    0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, // PUB
    0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, // SUB
    0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, // REQ
    0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, // REP
    0, 0, 0, 0, 1, 1, 1, 0, 0, 0, 0, // DEALER
    0, 0, 0, 1, 0, 1, 1, 0, 0, 0, 0, // ROUTER
    0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, // PULL
    0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, // PUSH
    0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, // XPUB
    0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, // XSUB
];

/// Checks if two sokets are compatible with each other
/// ```
/// use zeromq::util::sockets_compatible;
/// use zeromq::SocketType;
/// assert!(sockets_compatible(SocketType::PUB, SocketType::SUB));
/// assert!(sockets_compatible(SocketType::REQ, SocketType::REP));
/// assert!(sockets_compatible(SocketType::DEALER, SocketType::ROUTER));
/// assert!(!sockets_compatible(SocketType::PUB, SocketType::REP));
/// ```
pub fn sockets_compatible(one: SocketType, another: SocketType) -> bool {
    let row_index = one.to_usize().unwrap();
    let col_index = another.to_usize().unwrap();
    COMPATIBILITY_MATRIX[row_index * 11 + col_index] != 0
}

pub(crate) async fn greet_exchange(raw_socket: &mut FramedIo) -> ZmqResult<()> {
    raw_socket
        .send(Message::Greeting(ZmqGreeting::default()))
        .await?;

    let greeting: Option<CodecResult<Message>> = raw_socket.next().await;

    match greeting {
        Some(Ok(Message::Greeting(greet))) => match greet.version {
            (3, 0) => Ok(()),
            _ => Err(ZmqError::Other("Unsupported protocol version")),
        },
        _ => Err(ZmqError::Other("Failed Greeting exchange")),
    }
}

pub(crate) async fn ready_exchange(
    raw_socket: &mut FramedIo,
    socket_type: SocketType,
) -> ZmqResult<PeerIdentity> {
    let ready = ZmqCommand::ready(socket_type);
    raw_socket.send(Message::Command(ready)).await?;

    let ready_repl: Option<CodecResult<Message>> = raw_socket.next().await;
    match ready_repl {
        Some(Ok(Message::Command(command))) => match command.name {
            ZmqCommandName::READY => {
                let other_sock_type = command
                    .properties
                    .get("Socket-Type")
                    .map(|x| SocketType::try_from(x.as_str()))
                    .unwrap_or(Err(ZmqError::Other("Failed to parse other socket type")))?;

                let peer_id = command
                    .properties
                    .get("Identity")
                    .map_or_else(PeerIdentity::new, |x| {
                        x.clone().into_bytes().try_into().unwrap()
                    });

                if sockets_compatible(socket_type, other_sock_type) {
                    Ok(peer_id)
                } else {
                    Err(ZmqError::Other(
                        "Provided sockets combination is not compatible",
                    ))
                }
            }
        },
        Some(Ok(_)) => Err(ZmqError::Other("Failed to confirm ready state")),
        Some(Err(e)) => Err(e.into()),
        None => Err(ZmqError::Other("No reply from server")),
    }
}

pub(crate) async fn peer_connected(
    accept_result: ZmqResult<(FramedIo, Endpoint)>,
    backend: Arc<dyn MultiPeerBackend>,
) {
    let (mut raw_socket, _remote_endpoint) = accept_result.expect("Failed to accept");
    greet_exchange(&mut raw_socket)
        .await
        .expect("Failed to exchange greetings");
    let peer_id = ready_exchange(&mut raw_socket, backend.socket_type())
        .await
        .expect("Failed to exchange ready messages");

    let (outgoing_queue, stop_callback) = backend.peer_connected(&peer_id).await;

    // TODO: can we hold this handle somewhere to detect failure and clean up
    // properly?
    let _io_task_handle = tokio::spawn(async move {
        let mut stop_callback = stop_callback;
        let mut outgoing_queue = outgoing_queue;
        loop {
            tokio::select! {
                _ = &mut stop_callback => {
                    break;
                },
                outgoing = outgoing_queue.next() => {
                    match outgoing {
                        Some(message) => {
                            raw_socket.send(message).await.expect("Codec Error in send task");
                        },
                        None => {
                            break;
                        }
                    }
                },
                incoming = raw_socket.next() => {
                    match incoming {
                        Some(Ok(message)) => {
                            backend.message_received(&peer_id, message).await;
                        }
                        None => {
                            backend.peer_disconnected(&peer_id).await;
                            break;
                        }
                        _ => todo!(),
                    }
                },
            }
        }
    });
}

pub(crate) struct FairQueueProcessor {
    pub(crate) fair_queue_stream: FairQueue<mpsc::Receiver<Message>, PeerIdentity>,
    pub(crate) socket_incoming_queue: mpsc::Sender<(PeerIdentity, Message)>,
    pub(crate) peer_queue_in: mpsc::Receiver<(PeerIdentity, mpsc::Receiver<Message>)>,
    pub(crate) _io_close_handle: oneshot::Receiver<bool>,
}

pub(crate) async fn process_fair_queue_messages(mut processor: FairQueueProcessor) {
    let mut stop_callback = processor._io_close_handle;
    let mut waiting_for_clients = true;
    let mut waiting_for_data = true;
    loop {
        tokio::select! {
            _ = &mut stop_callback => {
                break;
            },
            peer_in = processor.peer_queue_in.next(), if waiting_for_clients => {
                match peer_in {
                    Some((peer_id, receiver)) => {
                        processor.fair_queue_stream.insert(peer_id, receiver);
                        waiting_for_data = true;
                    },
                    None => {
                        // Channel for newly connected clients was closed
                        // so we no longer wait for the to arrive
                        waiting_for_clients = false;
                    },
                };
            },
            message = processor.fair_queue_stream.next(), if waiting_for_data => {
                match message {
                    Some(m) => {
                        let result = processor.socket_incoming_queue.send(m).await;
                        match result {
                            Ok(()) => (),
                            Err(e) => {
                                if e.is_disconnected() {
                                    break;
                                }
                            }
                        }
                    },
                    None => {
                        // This is the case when there are no connected clients
                        // We should sleep and wait for new clients to connect
                        // This is handled by 2nd branch of select
                        if waiting_for_clients {
                            waiting_for_data = false;
                        } else {
                            // We're not waiting for client and have no data...
                            // stop the loop and cleanup
                            break;
                        };
                    }
                };
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::{Endpoint, Host, Socket, ZmqResult};

    pub async fn test_bind_to_unspecified_interface_helper(
        any: std::net::IpAddr,
        mut sock: impl Socket,
        start_port: u16,
    ) -> ZmqResult<()> {
        assert!(sock.binds().is_empty());
        assert!(any.is_unspecified());

        for i in 0..4 {
            sock.bind(
                Endpoint::Tcp(any.into(), start_port + i)
                    .to_string()
                    .as_str(),
            )
            .await?;
        }

        let bound_to = sock.binds();
        assert_eq!(bound_to.len(), 4);

        let mut port_set = std::collections::HashSet::new();
        for b in bound_to.keys() {
            if let Endpoint::Tcp(host, port) = b {
                assert_eq!(host, &any.into());
                port_set.insert(*port);
            } else {
                unreachable!()
            }
        }

        (start_port..start_port + 4).for_each(|p| assert!(port_set.contains(&p)));

        Ok(())
    }

    pub async fn test_bind_to_any_port_helper(mut sock: impl Socket) -> ZmqResult<()> {
        assert!(sock.binds().is_empty());
        for _ in 0..4 {
            sock.bind("tcp://localhost:0").await?;
        }

        let bound_to = sock.binds();
        assert_eq!(bound_to.len(), 4);
        let mut port_set = std::collections::HashSet::new();
        for b in bound_to.keys() {
            if let Endpoint::Tcp(host, port) = b {
                assert_eq!(host, &Host::Domain("localhost".to_string()));
                assert_ne!(*port, 0);
                // Insert and check that it wasn't already present
                assert!(port_set.insert(*port));
            } else {
                unreachable!()
            }
        }

        Ok(())
    }
}
