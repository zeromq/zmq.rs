use consts;
use socket_base::SocketBase;
use endpoint::Endpoint;


pub struct ReqSocket {
    destroying: bool,
}

impl SocketBase for ReqSocket {
    fn create() -> ReqSocket {
        ReqSocket {
            destroying: false,
        }
    }

    fn getsockopt(&self, option_: consts::SocketOption) -> int {
        match option_ {
            consts::TYPE => self.get_type() as int,
        }
    }

    fn get_type(&self) -> consts::SocketType {
        consts::REQ
    }

    fn add_endpoint(&self, endpoint: Box<Endpoint>) {
    }
}

