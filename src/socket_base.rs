use consts;


pub trait SocketBase {
    fn create() -> Self;

    fn getsockopt(&self, option_: consts::SocketOption) -> int;

    fn get_type(&self) -> consts::SocketType;

    fn bind(&self, addr: &str) -> Result<(), consts::ErrorCode> {
        parse_uri(addr).and_then(|(protocol, address)| {
            match protocol {
                "tcp" => Ok(()),
                _ => Err(consts::EINVAL),
            }
        })
    }
}

pub fn parse_uri<'r>(uri: &'r str) -> Result<(&'r str, &'r str), consts::ErrorCode> {
    match uri.find_str("://") {
        Some(pos) => {
            let protocol = uri.slice_to(pos);
            let address = uri.slice_from(pos + 3);
            if protocol.len() == 0 || address.len() == 0 {
                Err(consts::EINVAL)
            } else {
                Ok((protocol, address))
            }
        },
        None => Err(consts::EINVAL),
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

