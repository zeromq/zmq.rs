extern crate libc;


/// Defines different types of ØMQ sockets.
pub enum SocketType {
    //PAIR = 0,
    //PUB = 1,
    //SUB = 2,

    /// A socket of type `zeromq::REQ` is used by a *client* to send requests to and receive
    /// replies from a *service*. This socket type allows only an alternating sequence of
    /// `send(request)` and subsequent `recv(reply)` calls. Each request sent is round-robined
    /// among all `services`, and each reply received is matched with the last issued request.
    ///
    /// If no services are available, then any send operation on the socket shall block until at
    /// least one service becomes available. The `zeromq::REQ` socket shall not discard messages.
    REQ = 3,

    /// A socket of type `zeromq::REP` is used by a *service* to receive requests from and send
    /// replies to a *client*. This socket type allows only an alternating sequence of
    /// `recv(request)` and subsequent `send(reply)` calls. Each request received is fair-queued
    /// from among all *clients*, and each reply sent is routed to the *client* that issued the
    /// last request. If the original requester does not exist any more the reply is silently
    /// discarded.
    REP = 4,

    //DEALER = 5,
    //ROUTER = 6,
    //PULL = 7,
    //PUSH = 8,
    //XPUB = 9,
    //XSUB = 10,
    //STREAM = 11,
}

//impl Copy for SocketType {}


/// Defines different options for a ØMQ socket.
///
/// Option value can be retrieved from a socket by
/// [`getsockopt`](trait.ZmqSocket.html#tymethod.getsockopt).
pub enum SocketOption {
    /// *(readonly)* The `zeromq::TYPE` option shall retrieve the socket type for the specified
    /// *socket*. The socket type is specified at socket creation time and cannot be modified
    /// afterwards.
    TYPE = 16,
}

//impl Copy for SocketOption {}


/// A number random enough not to collide with different errno ranges on
/// different OSes. The assumption is that error_t is at least 32-bit type.
const HAUSNUMERO: isize = 156384712;

/// ØMQ errors.
#[derive(PartialEq, Debug)]
pub enum ErrorCode {
    /// Invalid argument
    EINVAL = libc::EINVAL as isize,

    /// Permission denied
    EACCES = libc::EACCES as isize,

    /// Connection refused
    ECONNREFUSED = libc::ECONNREFUSED as isize,

    /// Connection reset by peer
    ECONNRESET = libc::ECONNRESET as isize,

    /// Software caused connection abort
    ECONNABORTED = libc::ECONNABORTED as isize,

    /// Socket is not connected
    ENOTCONN = libc::ENOTCONN as isize,

    /// Connection timed out
    ETIMEDOUT = libc::ETIMEDOUT as isize,


    /// Protocol not supported
    EPROTONOSUPPORT = HAUSNUMERO + 2,

    /// Message too long
    EMSGSIZE = HAUSNUMERO + 10,

    /// Operation cannot be accomplished in current state
    EFSM = HAUSNUMERO + 51,

    /// Unknown I/O error
    EIOERROR = HAUSNUMERO - 1,
}

//impl Copy for ErrorCode {}

