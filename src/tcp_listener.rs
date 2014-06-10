use std;
use std::io::{Acceptor, Listener, TcpStream};
use std::io::net::ip::SocketAddr;
use result::{ZmqError, ZmqResult};
use endpoint::Endpoint;


static ACCEPT_TIMEOUT: u64 = 1000;


struct InnerTcpListener {
    addr: String,
    port: u16,
    chan: Sender<ZmqResult<TcpStream>>,
}


impl InnerTcpListener {
    fn run(&self) {
        let listener = std::io::TcpListener::bind(self.addr.as_slice(), self.port);
        let mut acceptor = listener.listen().unwrap();
        loop {
            acceptor.set_timeout(Some(ACCEPT_TIMEOUT));
            match acceptor.accept() {
                Ok(stream) => {
                    self.chan.send(Ok(stream));
                }
                Err(e) => {
                    self.chan.send(Err(ZmqError::from_io_error(e)));
                }
            }
        }
    }
}

pub struct TcpListener {
    chan: Receiver<ZmqResult<TcpStream>>,
}

impl TcpListener {
    pub fn new(addr: SocketAddr) -> TcpListener {
        let (tx, rx) = channel();
        spawn(proc() {
            let listener = InnerTcpListener {
                addr: format!("{}", addr.ip),
                port: addr.port,
                chan: tx,
            };
            listener.run();
        });

        TcpListener{
            chan: rx,
        }
    }
}

impl Endpoint for TcpListener {
}
