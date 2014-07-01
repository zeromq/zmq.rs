use options::Options;
use result::{ZmqError, ZmqResult};
use socket_base::{Ping, SocketMessage};
use stream_engine::StreamEngine;

use std::io::Acceptor;
use std::io::net::tcp::TcpAcceptor;
use std::sync::{RWLock, Arc};


static ACCEPT_TIMEOUT: u64 = 1000;


pub struct TcpListener {
    acceptor: TcpAcceptor,
    chan_to_socket: Sender<ZmqResult<SocketMessage>>,
    options: Arc<RWLock<Options>>,
}

impl TcpListener {
    fn run(&mut self) -> Result<(), ZmqResult<SocketMessage>> {
        loop {
            self.acceptor.set_timeout(Some(ACCEPT_TIMEOUT));
            match self.acceptor.accept() {
                Ok(stream) => {
                    if self.chan_to_socket.send_opt(Ok(Ping)).is_err() {
                        return Ok(());
                    }

                    let options = self.options.clone();
                    StreamEngine::spawn_new(stream, options, self.chan_to_socket.clone(), None);
                }
                Err(e) =>
                    try!(self.chan_to_socket.send_opt(Err(ZmqError::from_io_error(e)))),
            }
        }
    }

    pub fn spawn_new(acceptor: TcpAcceptor, chan: Sender<ZmqResult<SocketMessage>>,
                     options: Arc<RWLock<Options>>) {
        spawn(proc() {
            let mut listener = TcpListener {
                acceptor: acceptor,
                options: options,
                chan_to_socket: chan,
            };
            let _ = listener.run();
        });
    }
}
