use consts;
use result::{ZmqError, ZmqResult};
use std::io::net::ip::SocketAddr;
use tcp_listener::TcpListener;
use endpoint::Endpoint;


pub trait SocketBase {
    fn create() -> Self;

    fn getsockopt(&self, option_: consts::SocketOption) -> int;

    fn get_type(&self) -> consts::SocketType;

    fn add_endpoint(&self, endpoint: Box<Endpoint>);

    fn bind(&self, addr: &str) -> ZmqResult<()> {
        let (protocol, address) = try!(parse_uri(addr));
        try!(check_protocol(protocol));

        match protocol {
            "tcp" => {
                match from_str::<SocketAddr>(address) {
                    Some(addr) => {
                        self.add_endpoint(box TcpListener::new(addr));
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
