use endpoint::Endpoint;
use result::{ZmqError, ZmqResult};
use socket_base::{SocketBase, SocketMessage, OnConnected};
use stream_engine::StreamEngine;

use std::io::Acceptor;
use std::io::net::tcp::TcpAcceptor;


static ACCEPT_TIMEOUT: u64 = 1000;


struct InnerTcpListener {
    acceptor: TcpAcceptor,
    chan_to_socket: Sender<ZmqResult<SocketMessage>>,
}


impl InnerTcpListener {
    fn run(&mut self) -> Result<(), ZmqResult<SocketMessage>> {
        loop {
            self.acceptor.set_timeout(Some(ACCEPT_TIMEOUT));
            match self.acceptor.accept() {
                Ok(stream) =>
                    try!(self.chan_to_socket.send_opt(Ok(OnConnected(stream)))),
                Err(e) => {
                    try!(self.chan_to_socket.send_opt(Err(ZmqError::from_io_error(e))));
                }
            }
        }
    }
}


pub struct TcpListener {
    chan_from_inner: Receiver<ZmqResult<SocketMessage>>,
}

impl TcpListener {
    pub fn new(acceptor: TcpAcceptor) -> TcpListener {
        let (tx, rx) = channel();
        spawn(proc() {
            let mut listener = InnerTcpListener {
                acceptor: acceptor,
                chan_to_socket: tx,
            };
            match listener.run() {
                _ => ()
            }
        });

        TcpListener{
            chan_from_inner: rx,
        }
    }
}

impl Endpoint for TcpListener {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>> {
        &self.chan_from_inner
    }

    fn in_event(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut SocketBase) {
        match msg {
            Ok(OnConnected(stream)) => {
                let options = socket.clone_options();
                socket.add_endpoint(box StreamEngine::new(stream, options));
            }
            _ => ()
        }
    }
}
