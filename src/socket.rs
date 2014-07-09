use consts;
use msg::Msg;
use result::ZmqResult;


/// Interface to ØMQ socket objects.
///
/// # Key differences to conventional sockets
///
/// Generally speaking, conventional sockets present a *synchronous* interface to either
/// connection-oriented reliable byte streams (`SOCK_STREAM`), or connection-less unreliable
/// datagrams (`SOCK_DGRAM`). In comparison, ØMQ sockets present an abstraction of an asynchronous
/// *message queue*, with the exact queueing semantics depending on the socket type in use. Where
/// conventional sockets transfer streams of bytes or discrete datagrams, ØMQ sockets transfer
/// discrete *messages*.
///
/// ØMQ sockets being *asynchronous* means that the timings of the physical connection setup and
/// tear down, reconnect and effective delivery are transparent to the user and organized by ØMQ
/// itself. Further, messages may be *queued* in the event that a peer is unavailable to receive
/// them.
///
/// Conventional sockets allow only strict one-to-one (two peers), many-to-one (many clients, one
/// server), or in some cases one-to-many (multicast) relationships. With the exception of
/// *`zmq::PAIR`*, ØMQ sockets may be connected **to multiple endpoints** using *`connect()`*, while
/// simultaneously accepting incoming connections **from multiple endpoints** bound to the socket
/// using *`bind()`*, thus allowing many-to-many relationships.
pub trait ZmqSocket {
    /// This function shall retrieve the value for the option specified by the *`option`* argument
    /// for this ØMQ socket object and return it.
    fn getsockopt(&self, option: consts::SocketOption) -> int;

    /// This function binds the *socket* to a local endpoint and then accepts incoming connections
    /// on that endpoint.
    ///
    /// The *`endpoint`* is a string consisting of a *`transport://`* followed by an *`address`*.
    /// The *`transport`* specifies the underlying protocol to use. The *`address`* specifies the
    /// transport-specific address to bind to.
    ///
    /// ØMQ provides the the following transports:
    ///
    /// > ***`tcp`***
    /// >> unicast transport using TCP
    ///
    /// > ***`inproc`***
    /// >> local in-process (inter-task) communication transport
    ///
    /// Every ØMQ socket type except *`zmq::PAIR`* supports one-to-many and many-to-one semantics.
    ///
    /// # Errors
    ///
    /// > **`EINVAL`**
    /// >> The endpoint supplied is invalid.
    ///
    /// > **`EPROTONOSUPPORT`**
    /// >> The requested transport protocol is not supported.
    ///
    /// > **`ENOCOMPATPROTO`**
    /// >> The requested transport protocol is not compatible with the socket type.
    ///
    /// > **`EADDRINUSE`**
    /// >> The requested address is already in use.
    ///
    /// > **`EADDRNOTAVAIL`**
    /// >> The requested address was not local.
    ///
    /// > **`ENODEV`**
    /// >> The requested address specifies a nonexistent interface.
    ///
    /// > **`ETERM`**
    /// >> The ØMQ context associated with the specified socket was terminated.
    ///
    /// > **`ENOTSOCK`**
    /// >> The provided socket was invalid.
    ///
    /// # Example
    ///
    /// **Binding a publisher socket to an in-process and a tcp transport**
    ///
    /// ```rust
    /// // Create a zmq::PUB socket
    /// let socket = ctx.socket(zmq::PUB);
    /// // Bind it to a in-process transport with the address 'my_publisher'
    /// socket.bind("inproc://my_publisher");
    /// // Bind it to a TCP transport on port 5555 of the 'eth0' interface
    /// socket.bind("tcp://eth0:5555");
    /// ```
    fn bind(&self, endpoint: &str) -> ZmqResult<()>;

    /// This function connects the *socket* to an *endpoint* and then accepts incoming connections
    /// on that endpoint.
    ///
    /// The *endpoint* is a string consisting of a *`transport://`* followed by an *`address`*. The
    /// *`transport`* specifies the underlying protocol to use. The *`address`* specifies the
    /// transport-specific address to connect to.
    ///
    /// ØMQ provides the the following transports:
    ///
    /// > ***`tcp`***
    /// >> unicast transport using TCP
    ///
    /// > ***`inproc`***
    /// >> local in-process (inter-task) communication transport
    ///
    /// Every ØMQ socket type except *`zmq::PAIR`* supports one-to-many and many-to-one semantics.
    ///
    /// > for most transports and socket types the connection is not performed immediately but as
    /// > needed by ØMQ. Thus a successful call to *`connect()`* does not mean that the connection
    /// > was or could actually be established. Because of this, for most transports and socket
    /// > types the order in which a *server* socket is bound and a *client* socket is connected to
    /// > it does not matter. The first exception is when using the `inproc://` transport: you must
    /// > call *`bind()`* before calling *`connect()`*. The second exception are *`zmq::PAIR`*
    /// > sockets, which do not automatically reconnect to endpoints.
    ///
    /// # Errors
    ///
    /// > **`EINVAL`**
    /// >> The endpoint supplied is invalid.
    ///
    /// > **`EPROTONOSUPPORT`**
    /// >> The requested transport protocol is not supported.
    ///
    /// > **`ENOCOMPATPROTO`**
    /// >> The requested transport protocol is not compatible with the socket type.
    ///
    /// > **`ETERM`**
    /// >> The ØMQ context associated with the specified socket was terminated.
    ///
    /// > **`ENOTSOCK`**
    /// >> The provided socket was invalid.
    ///
    /// # Example
    ///
    /// **Connecting a subscriber socket to an in-process and a tcp transport**
    ///
    /// ```rust
    /// // Create a ZMQ_SUB socket
    /// let socket = ctx.socket(zmq::SUB);
    /// // Connect it to an in-process transport with the address 'my_publisher'
    /// socket.connect("inproc://my_publisher");
    /// // Connect it to the host server001, port 5555 using a TCP transport
    /// socket.connect("tcp://server001:5555");
    /// ```
    fn connect(&self, endpoint: &str) -> ZmqResult<()>;

    /// This function shall receive a message part from this socket and return it. If there are no
    /// message parts available on this socket the *`msg_recv()`* function shall block until the
    /// request can be satisfied.
    ///
    /// # Multi-part messages
    ///
    /// A ØMQ message is composed of 1 or more message parts. Each message part is an independent
    /// *`zmq::Msg`* in its own right. ØMQ ensures atomic delivery of messages: peers shall receive
    /// either all *message parts* of a message or none at all. The total number of message parts is
    /// unlimited except by available memory.
    ///
    /// # Errors
    ///
    /// > **`EAGAIN`**
    /// >> Non-blocking mode was requested and no messages are available at the moment.
    ///
    /// > **`ENOTSUP`**
    /// >> The *`msg_recv()`* operation is not supported by this socket type.
    ///
    /// > **`EFSM`**
    /// >> The *`msg_recv()`* operation cannot be performed on this socket at the moment due to the
    /// >> socket not being in the appropriate state. This error may occur with socket types that
    /// >> switch between several states, such as `zmq::REP`.
    ///
    /// > **`ETERM`**
    /// >> The ØMQ context associated with the specified socket was terminated.
    ///
    /// > **`ENOTSOCK`**
    /// >> The provided *socket* was invalid.
    ///
    /// > **`EINTR`**
    /// >> The operation was interrupted by delivery of a signal before a message was available.
    ///
    /// > **`EFAULT`**
    /// >> The message passed to the function was invalid.
    ///
    /// # Example
    ///
    /// **Receiving a message from a socket**
    ///
    /// ```rust
    /// // Block until a message is available to be received from socket
    /// let msg = socket.msg_recv();
    /// ```
    ///
    /// **Receiving a multi-part message**
    ///
    /// ```rust
    /// let mut more = true;
    /// while more {
    ///     // Block until a message is available to be received from socket
    ///     let msg = socket.msg_recv();
    ///     // Determine if more message parts are to follow
    ///     more = msg & 1 == 1;
    /// }
    /// ```
    fn msg_recv(&mut self) -> ZmqResult<Box<Msg>>;

    fn msg_send(&mut self, msg: Box<Msg>) -> ZmqResult<()>;
}
