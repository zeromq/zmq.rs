use consts;
use msg::Msg;
use peer::PeerManager;
use result::{ZmqError, ZmqResult};
use tcp_connecter::TcpConnecter;
use tcp_listener::TcpListener;

use std::io;
use std::io::Listener;
use std::io::net::ip::SocketAddr;


pub enum SocketMessage {
    Ping,
    OnConnected(Sender<Box<Msg>>, Receiver<Box<Msg>>),
}


pub trait SocketBase {
    fn new() -> Self;

    fn pm<'a>(&'a self) -> &'a PeerManager;

    fn pmut<'a>(&'a mut self) -> &'a mut PeerManager;

    fn init(self, type_: consts::SocketType) -> Self {
        self.pm().options.write().type_ = type_ as int;
        self
    }

    fn bind(&mut self, addr: &str) -> ZmqResult<()> {
        let (protocol, address) = try!(parse_uri(addr));

        match protocol {
            "tcp" => {
                match from_str::<SocketAddr>(address) {
                    Some(addr) => {
                        let listener = io::TcpListener::bind(
                            format!("{}", addr.ip).as_slice(), addr.port);
                        let acceptor = try!(listener.listen().map_err(ZmqError::from_io_error));
                        TcpListener::spawn_new(
                            acceptor, self.pm().tx.clone(), self.pm().options.clone());
                        Ok(())
                    }
                    None => Err(ZmqError::new(
                        consts::EINVAL, "Invaid argument: bad address")),
                }},
            _ => Err(ZmqError::new(consts::EPROTONOSUPPORT, "Protocol not supported")),
        }
    }

    fn connect(&mut self, addr: &str) -> ZmqResult<()> {
        let (protocol, address) = try!(parse_uri(addr));
        match protocol {
            "tcp" => {
                match from_str::<SocketAddr>(address) {
                    Some(addr) => {
                        TcpConnecter::spawn_new(
                            addr, self.pm().tx.clone(), self.pm().options.clone());
                        Ok(())
                    }
                    None => Err(ZmqError::new(
                        consts::EINVAL, "Invaid argument: bad address")),
                }},
            _ => Err(ZmqError::new(consts::EPROTONOSUPPORT, "Protocol not supported")),
        }
    }

    fn getsockopt(&self, option: consts::SocketOption) -> int {
        self.pm().options.read().getsockopt(option)
    }

    fn msg_recv(&mut self) -> ZmqResult<Box<Msg>>;

    fn msg_send(&mut self, mut msg: Box<Msg>) -> ZmqResult<()>;
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
