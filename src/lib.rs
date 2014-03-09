#[crate_id = "zmq.rs#0.1-pre"];
#[crate_type = "dylib"];
#[license = "MIT"];

pub use ctx::Context;
pub use consts::{PAIR, PUB, SUB, REQ, REP, DEALER, ROUTER, PULL, PUSH, XPUB, XSUB, STREAM};

mod ctx;
mod consts;
mod socket_base;


#[cfg(test)]
mod test {
    #[test]
    fn test_socket_type() {
        assert_eq!(super::REQ as int, 3);
    }
}

