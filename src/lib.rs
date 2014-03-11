#[crate_id = "zmq.rs#0.1-pre"];
#[crate_type = "dylib"];
#[license = "MIT"];

pub use ctx::Context;
pub use consts::{SocketType, REQ};
pub use consts::{SocketOption, TYPE};
pub use consts::{ErrorCode, EINVAL};
pub use socket_base::SocketBase;

mod ctx;
mod consts;
mod socket_base;
mod req;


#[cfg(test)]
mod test {
    #[test]
    fn test_socket_type() {
        assert_eq!(super::REQ as int, 3);
    }

    #[test]
    fn test_socket_create() {
        let c = super::Context::new();
        let s = c.socket(super::REQ);
        assert_eq!(s.getsockopt(super::TYPE), super::REQ as int);
        match s.get_type() {
            super::REQ => (),
            //_ => assert!(false),
        }
    }

    #[test]
    fn test_socket_bind() {
        let c = super::Context::new();
        let s = c.socket(super::REQ);
        assert!(s.bind("").is_err());
        assert!(s.bind("://127").is_err());
        assert!(s.bind("tcp://").is_err());
        assert!(s.bind("tcp://10.0.1.1:12345").is_ok());
    }
}

