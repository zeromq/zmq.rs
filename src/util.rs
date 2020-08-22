use crate::*;
use bytes::Bytes;
use futures::lock::Mutex;
use futures::stream::StreamExt;
use futures::{select, SinkExt};
use futures_util::future::FutureExt;
use std::convert::{TryFrom, TryInto};
use std::sync::Arc;
use tokio::net::TcpStream;
use uuid::Uuid;

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Clone)]
pub(crate) struct PeerIdentity(Vec<u8>);

impl PeerIdentity {
    pub fn new() -> Self {
        let id = Uuid::new_v4();
        Self(id.as_bytes().to_vec())
    }
}

impl TryFrom<Vec<u8>> for PeerIdentity {
    type Error = ZmqError;

    fn try_from(data: Vec<u8>) -> Result<Self, ZmqError> {
        if data.len() == 0 {
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
    pub(crate) identity: PeerIdentity,
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
/// use zeromq::SocketType;
/// use zeromq::util::sockets_compatible;
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

pub(crate) async fn greet_exchange(socket: &mut Framed<TcpStream, ZmqCodec>) -> ZmqResult<()> {
    socket
        .send(Message::Greeting(ZmqGreeting::default()))
        .await?;

    let greeting: Option<Result<Message, ZmqError>> = socket.next().await;

    match greeting {
        Some(Ok(Message::Greeting(greet))) => match greet.version {
            (3, 0) => Ok(()),
            _ => Err(ZmqError::Other("Unsupported protocol version")),
        },
        _ => Err(ZmqError::Codec("Failed Greeting exchange")),
    }
}

pub(crate) async fn ready_exchange(
    socket: &mut Framed<TcpStream, ZmqCodec>,
    socket_type: SocketType,
) -> ZmqResult<PeerIdentity> {
    let ready = ZmqCommand::ready(socket_type);
    socket.send(Message::Command(ready)).await?;

    let ready_repl: Option<ZmqResult<Message>> = socket.next().await;
    match ready_repl {
        Some(Ok(Message::Command(command))) => match command.name {
            ZmqCommandName::READY => {
                let other_sock_type = command
                    .properties
                    .get("Socket-Type")
                    .map(|x| SocketType::try_from(x.as_str()))
                    .unwrap_or(Err(ZmqError::Codec("Failed to parse other socket type")))?;

                let peer_id = command.properties.get("Identity").map_or_else(
                    || PeerIdentity::new(),
                    |x| x.clone().into_bytes().try_into().unwrap(),
                );

                if sockets_compatible(socket_type, other_sock_type) {
                    Ok(peer_id)
                } else {
                    Err(ZmqError::Other(
                        "Provided sockets combination is not compatible",
                    ))
                }
            }
        },
        Some(Ok(_)) => Err(ZmqError::Codec("Failed to confirm ready state")),
        Some(Err(e)) => Err(e),
        None => Err(ZmqError::Other("No reply from server")),
    }
}

pub(crate) async fn raw_connect(
    socket_type: SocketType,
    endpoint: &str,
) -> ZmqResult<Framed<TcpStream, ZmqCodec>> {
    let addr = endpoint.parse::<SocketAddr>()?;
    let mut raw_socket = Framed::new(TcpStream::connect(addr).await?, ZmqCodec::new());
    greet_exchange(&mut raw_socket).await?;
    ready_exchange(&mut raw_socket, socket_type).await?;
    Ok(raw_socket)
}

pub(crate) async fn peer_connected(socket: tokio::net::TcpStream, backend: Arc<dyn MultiPeer>) {
    let mut raw_socket = Framed::new(socket, ZmqCodec::new());

    greet_exchange(&mut raw_socket)
        .await
        .expect("Failed to exchange greetings");
    let peer_id = ready_exchange(&mut raw_socket, backend.socket_type())
        .await
        .expect("Failed to exchange ready messages");
    println!("Peer connected {:?}", peer_id);

    let (outgoing_queue, stop_callback) = backend.peer_connected(&peer_id).await;

    tokio::spawn(async move {
        let mut stop_callback = stop_callback.fuse();
        let mut outgoing_queue = outgoing_queue.fuse();
        loop {
            tokio::select! {
                _ = &mut stop_callback => {
                    println!("Stop callback received");
                    break;
                },
                outgoing = outgoing_queue.next() => {
                    match outgoing {
                        Some(message) => {
                            let result = raw_socket.send(message).await;
                            if let Err(e) = result {
                                println!("{}", e);
                                break;
                            }
                        },
                        None => {
                            println!("Outgoing queue closed. Stopping send coro");
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

/// Opens port described by endpoint and starts a coroutine to accept new connections on it
/// Returns stop_handle channel that can be used to stop accepting new connections
pub(crate) async fn start_accepting_connections(
    endpoint: &str,
    backend: Arc<dyn MultiPeer>,
) -> ZmqResult<futures::channel::oneshot::Sender<bool>> {
    let mut listener = tokio::net::TcpListener::bind(endpoint).await?;
    let (stop_handle, stop_callback) = futures::channel::oneshot::channel::<bool>();
    tokio::spawn(async move {
        let mut stop_callback = stop_callback.fuse();
        loop {
            select! {
                incoming = listener.accept().fuse() => {
                    let (socket, _) = incoming.expect("Failed to accept connection");
                    tokio::spawn(peer_connected(socket, backend.clone()));
                },
                _ = stop_callback => {
                    println!("Stop signal received");
                    break
                }
            }
        }
    });
    Ok(stop_handle)
}
