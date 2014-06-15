use consts;
use std::collections::{HashMap, DList, Deque};
use std::io::{Listener, TcpStream, TcpListener};
use std::io::net::ip::SocketAddr;
use std::io::net::tcp::TcpAcceptor;
use std::comm::Select;

use tcp_listener;
use endpoint::Endpoint;
use result::{ZmqError, ZmqResult};


pub enum SocketMessage {
    DoBind(TcpAcceptor),
    OnConnected(TcpStream),
}


struct InnerSocketBase {
    chan: Receiver<ZmqResult<SocketMessage>>,
}

impl Endpoint for InnerSocketBase {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>> {
        &self.chan
    }

    fn handle(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut InnerSocket) {
        match msg {
            Ok(DoBind(acceptor)) => {
                socket.add_endpoint(box tcp_listener::TcpListener::new(acceptor));
            }
            _ => ()
        }
    }
}


// TODO: this is a bad naming
pub struct InnerSocket {
    endpoints: DList<Box<Endpoint>>,
}

impl InnerSocket {
    fn new() -> InnerSocket {
        InnerSocket {
            endpoints: DList::new(),
        }
    }

    fn run(&mut self) {
        loop {
            // in order to call `handle` with a mutable `self`, we have to move the endpoints
            // TODO: this doesn't look cool, we need a better solution
            let mut endpoints = Vec::with_capacity(self.endpoints.len());
            loop {
                match self.endpoints.pop_front() {
                    Some(endpoint) => {
                        endpoints.push(endpoint);
                    }
                    None => break
                }
            }
            let (msg, index) = {
                let selector = Select::new();
                let mut mapping = HashMap::new();
                let mut index = 0;
                for endpoint in endpoints.iter() {
                    let handle = box selector.handle(endpoint.get_chan());
                    let hid = handle.id();
                    mapping.insert(hid, (handle, index));
                    let handle = mapping.get_mut(&hid).mut0();
                    unsafe {
                        handle.add();
                    }
                    index += 1;
                }
                let hid = selector.wait();
                match mapping.pop(&hid) {
                    Some((mut handle, index)) => {
                        match handle.recv_opt() {
                            Ok(msg) => (msg, index),
                            _ => break,
                        }
                    }
                    None => fail!(),
                }
            };
            endpoints.get_mut(index).handle(msg, self);
            self.endpoints.extend(endpoints.move_iter());
        }
    }

    fn add_endpoint(&mut self, endpoint: Box<Endpoint>) {
        self.endpoints.push_back(endpoint);
    }
}


pub trait SocketBase {
    fn create() -> Self;

    fn _get_chan<'a>(&'a self) -> &'a Sender<ZmqResult<SocketMessage>>;

    fn getsockopt(&self, option_: consts::SocketOption) -> int;

    fn get_type(&self) -> consts::SocketType;

    fn _init(&self, chan: Receiver<ZmqResult<SocketMessage>>) {
        spawn(proc() {
            let mut socket = InnerSocket::new();
            socket.add_endpoint(box InnerSocketBase {
                chan: chan,
            });
            socket.run();
        });
    }

    fn _send_command(&self, cmd: SocketMessage) {
        self._get_chan().send(Ok(cmd));
    }

    fn bind(&self, addr: &str) -> ZmqResult<()> {
        let (protocol, address) = try!(parse_uri(addr));
        try!(check_protocol(protocol));

        match protocol {
            "tcp" => {
                match from_str::<SocketAddr>(address) {
                    Some(addr) => {
                        let listener = TcpListener::bind(
                            format!("{}", addr.ip).as_slice(), addr.port);
                        let acceptor = try!(ZmqError::wrap_io_error(listener.listen()));
                        self._send_command(DoBind(acceptor));
                        Ok(())
                    }
                    None => Err(ZmqError::new(
                        consts::EINVAL, "Invaid argument: bad address")),
                }},
            _ => Ok(())
        }
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

fn check_protocol(protocol: &str) -> ZmqResult<()> {
    match protocol {
        "tcp" => Ok(()),
        _ => Err(ZmqError::new(consts::EPROTONOSUPPORT, "Protocol not supported")),
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
