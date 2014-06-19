use consts;
use endpoint::Endpoint;
use result::{ZmqError, ZmqResult};
use socket_base::{DoBind, SocketMessage};
use socket_base::SocketBase;
use tcp_listener::TcpListener;

use std::io;
use std::io::Listener;
use std::io::net::ip::SocketAddr;


struct InnerZmqSocket {
    rx: Receiver<ZmqResult<SocketMessage>>,
}

impl InnerZmqSocket {
    fn new(rx: Receiver<ZmqResult<SocketMessage>>) -> InnerZmqSocket {
        InnerZmqSocket {
            rx: rx,
        }
    }
}

impl Endpoint for InnerZmqSocket {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>> {
        &self.rx
    }

    fn in_event(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut SocketBase) {
        match msg {
            Ok(DoBind(acceptor)) => {
                socket.add_endpoint(box TcpListener::new(acceptor));
            }
            _ => ()
        }
    }
}


pub struct ZmqSocket {
    type_: consts::SocketType,
    tx: Sender<ZmqResult<SocketMessage>>,
}

impl ZmqSocket {
    pub fn new(type_: consts::SocketType) -> ZmqSocket {
        let (tx, rx) = channel();
        spawn(proc() {
            let mut socket = SocketBase::new();
            let endpoint = box InnerZmqSocket::new(rx);
            socket.add_endpoint(endpoint);
            socket.run();
        });
        ZmqSocket {
            type_: type_,
            tx: tx,
        }
    }

    pub fn bind(&self, addr: &str) -> ZmqResult<()> {
        let (protocol, address) = try!(parse_uri(addr));

        match protocol {
            "tcp" => {
                match from_str::<SocketAddr>(address) {
                    Some(addr) => {
                        let listener = io::TcpListener::bind(
                            format!("{}", addr.ip).as_slice(), addr.port);
                        let acceptor = try!(ZmqError::wrap_io_error(listener.listen()));
                        self.tx.send(Ok(DoBind(acceptor)));
                        Ok(())
                    }
                    None => Err(ZmqError::new(
                        consts::EINVAL, "Invaid argument: bad address")),
                }},
            _ => Err(ZmqError::new(consts::EPROTONOSUPPORT, "Protocol not supported")),
        }
    }

    pub fn getsockopt(&self, option_: consts::SocketOption) -> int {
        match option_ {
            consts::TYPE => self.get_type() as int,
        }
    }

    pub fn get_type(&self) -> consts::SocketType {
        consts::REQ
    }
}


fn parse_uri<'r>(uri: &'r str) -> ZmqResult<(&'r str, &'r str)> {
    match uri.find_str("://") {
        Some(pos) => {
            let protocol = uri.slice_to(pos);
            let address = uri.slice_from(pos + 3);
            if protocol.len() == 0 || address.len() == 0 {
                Err(ZmqError::new(
                    consts::EINVAL,
                    "Invalid argument: missing protocol or address"))
            } else {
                Ok((protocol, address))
            }
        },
        None => Err(ZmqError::new(
            consts::EINVAL, "Invalid argument: missing ://")),
    }
}


#[cfg(test)]
mod test {
    use super::parse_uri;

    #[test]
    fn test_parse_uri() {
        assert!(parse_uri("").is_err());
        assert!(parse_uri("://").is_err());
        assert!(parse_uri("tcp://").is_err());
        assert!(parse_uri("://127.0.0.1").is_err());
        match parse_uri("tcp://127.0.0.1:8890") {
            Ok((protocol, address)) => {
                assert_eq!(protocol, "tcp");
                assert_eq!(address, "127.0.0.1:8890");
            },
            Err(_) => {assert!(false);},
        }
    }
}
