use crate::*;
use tokio::net::TcpStream;

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
/// use zmq_rs::SocketType;
/// use zmq_rs::util::sockets_compatible;
/// assert!(sockets_compatible(SocketType::PUB, SocketType::SUB));
/// assert!(sockets_compatible(SocketType::REQ, SocketType::REP));
/// assert!(sockets_compatible(SocketType::DEALER, SocketType::ROUTER));
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

pub(crate) async fn greet_exchange_w_parts(
    sink: &mut tokio_util::codec::FramedWrite<
        tokio::io::WriteHalf<tokio::net::TcpStream>,
        ZmqCodec,
    >,
    stream: &mut tokio_util::codec::FramedRead<
        tokio::io::ReadHalf<tokio::net::TcpStream>,
        ZmqCodec,
    >,
) -> ZmqResult<()> {
    sink.send(Message::Greeting(ZmqGreeting::default())).await?;

    let greeting: Option<Result<Message, ZmqError>> = stream.next().await;

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
) -> ZmqResult<()> {
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

                if sockets_compatible(socket_type, other_sock_type) {
                    Ok(())
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

pub(crate) async fn ready_exchange_w_parts(
    sink: &mut tokio_util::codec::FramedWrite<
        tokio::io::WriteHalf<tokio::net::TcpStream>,
        ZmqCodec,
    >,
    stream: &mut tokio_util::codec::FramedRead<
        tokio::io::ReadHalf<tokio::net::TcpStream>,
        ZmqCodec,
    >,
    socket_type: SocketType,
) -> ZmqResult<()> {
    let ready = ZmqCommand::ready(socket_type);
    sink.send(Message::Command(ready)).await?;

    let ready_repl: Option<ZmqResult<Message>> = stream.next().await;
    match ready_repl {
        Some(Ok(Message::Command(command))) => match command.name {
            ZmqCommandName::READY => {
                let other_sock_type = command
                    .properties
                    .get("Socket-Type")
                    .map(|x| SocketType::try_from(x.as_str()))
                    .unwrap_or(Err(ZmqError::Codec("Failed to parse other socket type")))?;

                if sockets_compatible(socket_type, other_sock_type) {
                    Ok(())
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
