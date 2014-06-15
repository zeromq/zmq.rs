use std::io::Acceptor;
use std::io::net::tcp::TcpAcceptor;

use endpoint::Endpoint;
use result::{ZmqError, ZmqResult};
use socket_base::{InnerSocket, SocketMessage, OnConnected};


static ACCEPT_TIMEOUT: u64 = 1000;


struct InnerTcpListener {
    acceptor: TcpAcceptor,
    chan: Sender<ZmqResult<SocketMessage>>,
}


impl InnerTcpListener {
    fn run(&mut self) -> Result<(), ZmqResult<SocketMessage>> {
        loop {
            self.acceptor.set_timeout(Some(ACCEPT_TIMEOUT));
            match self.acceptor.accept() {
                Ok(stream) =>
                    try!(self.chan.send_opt(Ok(OnConnected(stream)))),
                Err(e) => {
                    try!(self.chan.send_opt(Err(ZmqError::from_io_error(e))));
                }
            }
        }
    }
}

pub struct TcpListener {
    chan: Receiver<ZmqResult<SocketMessage>>,
}

impl TcpListener {
    pub fn new(acceptor: TcpAcceptor) -> TcpListener {
        let (tx, rx) = channel();
        spawn(proc() {
            let mut listener = InnerTcpListener {
                acceptor: acceptor,
                chan: tx,
            };
            match listener.run() {
                _ => ()
            }
        });

        TcpListener{
            chan: rx,
        }
    }
}

impl Endpoint for TcpListener {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>> {
        &self.chan
    }

    fn handle(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut InnerSocket) {
    }
}
