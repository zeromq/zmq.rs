#![crate_id = "zmq#0.1.0-pre"]
#![crate_type = "rlib"]
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
mod tcp_connecter;
mod tcp_listener;
mod options;
mod v2_encoder;
mod v2_decoder;
mod v2_protocol;


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
        let s = c.socket(super::REQ);
        assert_eq!(s.bind("").unwrap_err().code, super::EINVAL);
        assert_eq!(s.bind("://127").unwrap_err().code, super::EINVAL);
        assert_eq!(s.bind("tcp://").unwrap_err().code, super::EINVAL);
        assert_eq!(s.bind("tcpp://127.0.0.1:12345").unwrap_err().code, super::EPROTONOSUPPORT);
        assert_eq!(s.bind("tcp://10.0.1.255:12345").unwrap_err().code, super::ECONNREFUSED);
        assert_eq!(s.bind("tcp://10.0.1.1:12z45").unwrap_err().code, super::EINVAL);
        assert!(s.bind("tcp://127.0.0.1:12345").is_ok());
        assert_eq!(s.bind("tcp://127.0.0.1:12345").unwrap_err().code, super::ECONNREFUSED);
    }

    #[test]
    fn test_socket_connect() {
        let c = super::Context::new();
        let s = c.socket(super::REQ);
        assert_eq!(s.connect("").unwrap_err().code, super::EINVAL);
        assert_eq!(s.connect("://127").unwrap_err().code, super::EINVAL);
        assert_eq!(s.connect("tcp://").unwrap_err().code, super::EINVAL);
        assert_eq!(s.connect("tcpp://127.0.0.1:12346").unwrap_err().code, super::EPROTONOSUPPORT);
        assert_eq!(s.connect("tcp://10.0.1.1:12z46").unwrap_err().code, super::EINVAL);
        assert!(s.connect("tcp://127.0.0.1:12346").is_ok());
        assert!(s.connect("tcp://127.0.0.1:12346").is_ok());
    }

    #[test]
    fn test_socket_small_message() {
        let c = super::Context::new();
        let mut req = c.socket(super::REQ);
        let mut rep = c.socket(super::REQ);
        assert!(rep.bind("tcp://127.0.0.1:12347").is_ok());
        assert!(req.connect("tcp://127.0.0.1:12347").is_ok());

        let mut msg_sent = box super::Msg::new(4);
        msg_sent.data.push_all([65u8, 66u8, 67u8, 68u8]);
        req.msg_send(msg_sent);

        let msg_recv = rep.msg_recv();
        assert_eq!(msg_recv.data, [65u8, 66u8, 67u8, 68u8].into_owned());
    }
}
