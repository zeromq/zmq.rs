use endpoint::Endpoint;
use options::Options;
use result::{ZmqError, ZmqResult};
use socket_base::{SocketBase, SocketMessage, OnConnected};
use stream_engine::StreamEngine;

use std::cmp;
use std::io::net::ip::SocketAddr;
use std::io::{TcpStream, timer};
use std::rand;
use std::sync::{RWLock, Arc};


struct InnerTcpConnecter {
    chan_to_socket: Sender<ZmqResult<SocketMessage>>,
    addr: SocketAddr,
    options: Arc<RWLock<Options>>,

    //  Current reconnect ivl, updated for backoff strategy
    current_reconnect_ivl: u64,
}

impl InnerTcpConnecter {
    fn run(&mut self) -> Result<(), ZmqResult<SocketMessage>> {
        loop {
            match TcpStream::connect(format!("{}", self.addr.ip).as_slice(), self.addr.port) {
                Ok(stream) => {
                    let (tx, rx) = channel();
                    try!(self.chan_to_socket.send_opt(Ok(OnConnected(stream, Some(tx)))));
                    let _ = rx.recv_opt();
                }
                Err(e) =>
                    try!(self.chan_to_socket.send_opt(Err(ZmqError::from_io_error(e)))),
            }

            let reconnect_ivl = self.options.read().reconnect_ivl;
            let reconnect_ivl_max = self.options.read().reconnect_ivl_max;

            //  The new interval is the current interval + random value.
            let interval = self.current_reconnect_ivl + rand::random::<u64>() % reconnect_ivl;

            //  Only change the current reconnect interval  if the maximum reconnect
            //  interval was set and if it's larger than the reconnect interval.
            if reconnect_ivl_max > 0 && reconnect_ivl_max > reconnect_ivl {
                //  Calculate the next interval
                self.current_reconnect_ivl =
                    cmp::min (self.current_reconnect_ivl * 2, reconnect_ivl_max);
            }

            timer::sleep(interval);
        }
    }
}


pub struct TcpConnecter {
    chan_from_inner: Receiver<ZmqResult<SocketMessage>>,
}

impl TcpConnecter {
    pub fn new(addr: SocketAddr, options: Arc<RWLock<Options>>) -> TcpConnecter {
        let (tx, rx) = channel();
        spawn(proc() {
            let reconnect_ivl = options.read().reconnect_ivl;
            let mut connecter = InnerTcpConnecter {
                chan_to_socket: tx,
                addr: addr,
                options: options,
                current_reconnect_ivl: reconnect_ivl,
            };
            let _ = connecter.run();
        });
        TcpConnecter {
            chan_from_inner: rx,
        }
    }
}

impl Endpoint for TcpConnecter {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>> {
        &self.chan_from_inner
    }

    fn in_event(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut SocketBase) {
        match msg {
            Ok(OnConnected(stream, notifier)) => {
                let options = socket.clone_options();
                socket.add_endpoint(box StreamEngine::new(stream, options, notifier));
            }
            _ => ()
        }
    }
}
