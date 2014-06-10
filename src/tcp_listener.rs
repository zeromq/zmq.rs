use std;
use std::io::{Acceptor, Listener, TcpStream};
use std::io::net::tcp::TcpAcceptor;
use std::io::net::ip::SocketAddr;
use result::{ZmqError, ZmqResult};
use endpoint::Endpoint;


static ACCEPT_TIMEOUT: u64 = 1000;


struct InnerTcpListener {
    acceptor: TcpAcceptor,
    chan: Sender<ZmqResult<TcpStream>>,
}


impl InnerTcpListener {
    fn run(&mut self) -> Result<(), ZmqResult<TcpStream>> {
        loop {
            self.acceptor.set_timeout(Some(ACCEPT_TIMEOUT));
            match self.acceptor.accept() {
                Ok(stream) =>
                    try!(self.chan.send_opt(Ok(stream))),
                Err(e) =>
                    try!(self.chan.send_opt(Err(ZmqError::from_io_error(e)))),
            }
        }
    }
}

pub struct TcpListener {
    chan: Receiver<ZmqResult<TcpStream>>,
}

impl TcpListener {
    pub fn new(addr: SocketAddr) -> ZmqResult<TcpListener> {
        let listener = std::io::TcpListener::bind(
            format!("{}", addr.ip).as_slice(), addr.port);
        let acceptor = try!(ZmqError::wrap_io_error(listener.listen()));
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

        Ok(TcpListener{
            chan: rx,
        })
    }
}

impl Endpoint for TcpListener {
}
