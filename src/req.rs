use consts;
use socket_base::{SocketBase, SocketMessage};
use result::ZmqResult;


pub struct ReqSocket {
    destroying: bool,
    chan: Sender<ZmqResult<SocketMessage>>,
}

impl SocketBase for ReqSocket {
    fn create() -> ReqSocket {
        let (tx, rx) = channel();
        let ret = ReqSocket {
            destroying: false,
            chan: tx,
        };
        ret._init(rx);
        ret
    }

    fn getsockopt(&self, option_: consts::SocketOption) -> int {
        match option_ {
            consts::TYPE => self.get_type() as int,
        }
    }

    fn get_type(&self) -> consts::SocketType {
        consts::REQ
    }

    fn _get_chan<'a>(&'a self) -> &'a Sender<ZmqResult<SocketMessage>> {
        &self.chan
    }
}
