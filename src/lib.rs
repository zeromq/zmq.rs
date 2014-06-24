#![crate_id = "zmq.rs#0.1-pre"]
#![crate_type = "dylib"]
#![license = "MPLv2"]

pub use ctx::Context;
pub use consts::{SocketType, REQ};
pub use consts::{SocketOption, TYPE};
pub use consts::{HAUSNUMERO, ErrorCode, EINVAL, EPROTONOSUPPORT, ECONNREFUSED};
pub use msg::Msg;
pub use result::{ZmqResult, ZmqError};
pub use socket_interface::ZmqSocket;

mod ctx;
mod consts;
mod endpoint;
mod msg;
mod result;
mod socket_base;
mod socket_interface;
mod stream_engine;
mod tcp_listener;
mod options;
mod v2_decoder;


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
    }

    #[test]
    fn test_socket_bind() {
        let c = super::Context::new();
        let mut s = c.socket(super::REQ);
        assert_eq!(s.bind("").unwrap_err().code, super::EINVAL);
        assert_eq!(s.bind("://127").unwrap_err().code, super::EINVAL);
        assert_eq!(s.bind("tcp://").unwrap_err().code, super::EINVAL);
        assert_eq!(s.bind("tcpp://127.0.0.1:12345").unwrap_err().code, super::EPROTONOSUPPORT);
        assert_eq!(s.bind("tcp://10.0.1.255:12345").unwrap_err().code, super::ECONNREFUSED);
        assert_eq!(s.bind("tcp://10.0.1.1:12z45").unwrap_err().code, super::EINVAL);
        assert!(s.bind("tcp://127.0.0.1:12345").is_ok());
        /*loop {
            println!(">>> {}", s.msg_recv());
        }*/
    }
}
