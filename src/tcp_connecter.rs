use options::Options;
use result::{ZmqError, ZmqResult};
use socket_base::SocketMessage;
use stream_engine::StreamEngine;

use std::cmp;
use std::io::net::ip::SocketAddr;
use std::io::{TcpStream, timer};
use std::num::SignedInt;
use std::rand;
use std::sync::{RWLock, Arc};
use std::time::duration::Duration;


pub struct TcpConnecter {
    chan_to_socket: Sender<ZmqResult<SocketMessage>>,
    addr: SocketAddr,
    options: Arc<RWLock<Options>>,

    //  Current reconnect ivl, updated for backoff strategy
    current_reconnect_ivl: Duration,
}

impl TcpConnecter {
    fn run(&mut self) -> Result<(), ZmqResult<SocketMessage>> {
        loop {
            match TcpStream::connect(format!("{}:{}", self.addr.ip, self.addr.port).as_slice()) {
                Ok(stream) => {
                    if self.chan_to_socket.send_opt(Ok(SocketMessage::Ping)).is_err() {
                        return Ok(());
                    }

                    let (tx, rx) = channel();
                    StreamEngine::spawn_new(
                        stream, self.options.clone(), self.chan_to_socket.clone(), Some(tx));
                    let _ = rx.recv_opt();
                }
                Err(e) =>
                    try!(self.chan_to_socket.send_opt(Err(ZmqError::from_io_error(e)))),
            }

            let reconnect_ivl = self.options.read().reconnect_ivl;
            let reconnect_ivl_max = self.options.read().reconnect_ivl_max;

            //  The new interval is the current interval + random value.
            let interval = self.current_reconnect_ivl + Duration::milliseconds(
                (rand::random::<i64>() % reconnect_ivl.num_milliseconds()).abs());

            //  Only change the current reconnect interval  if the maximum reconnect
            //  interval was set and if it's larger than the reconnect interval.
            if reconnect_ivl_max > Duration::zero() && reconnect_ivl_max > reconnect_ivl {
                //  Calculate the next interval
                self.current_reconnect_ivl =
                    cmp::min (self.current_reconnect_ivl * 2, reconnect_ivl_max);
            }

            timer::sleep(interval);
        }
    }

    pub fn spawn_new(addr: SocketAddr, chan: Sender<ZmqResult<SocketMessage>>,
                     options: Arc<RWLock<Options>>) {
        spawn(move || {
            let reconnect_ivl = options.read().reconnect_ivl;
            let mut connecter = TcpConnecter {
                chan_to_socket: chan,
                addr: addr,
                options: options,
                current_reconnect_ivl: reconnect_ivl,
            };
            let _ = connecter.run();
        });
    }
}
