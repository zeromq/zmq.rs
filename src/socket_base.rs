use consts;
use result::{ZmqError, ZmqResult};
use std::io::net::ip::SocketAddr;


pub trait SocketBase {
    fn create() -> Self;

    fn getsockopt(&self, option_: consts::SocketOption) -> int;

    fn get_type(&self) -> consts::SocketType;

    fn bind(&self, addr: &str) -> ZmqResult<()> {
        let (protocol, address) = try!(parse_uri(addr));
        try!(check_protocol(protocol));

        match from_str::<SocketAddr>(address) {
            Some(_) => {
                Ok(())
            }
            None => Err(ZmqError{
                code: consts::EINVAL,
                desc: "Invaid argument: bad address",
                detail: None,
            }),
        }
    }
}

fn parse_uri<'r>(uri: &'r str) -> ZmqResult<(&'r str, &'r str)> {
    match uri.find_str("://") {
        Some(pos) => {
            let protocol = uri.slice_to(pos);
            let address = uri.slice_from(pos + 3);
            if protocol.len() == 0 || address.len() == 0 {
                Err(ZmqError{
                    code: consts::EINVAL,
                    desc: "Invalid argument: missing protocol or address",
                    detail: None,
                })
            } else {
                Ok((protocol, address))
            }
        },
        None => Err(ZmqError{
            code: consts::EINVAL,
            desc: "Invalid argument: missing ://",
            detail: None,
        }),
    }
}

fn check_protocol(protocol: &str) -> ZmqResult<()> {
    match protocol {
        "tcp" => Ok(()),
        _ => Err(ZmqError{
            code: consts::EPROTONOSUPPORT,
            desc: "Protocol not supported",
            detail: None,
        }),
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
