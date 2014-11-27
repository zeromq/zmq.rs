use consts::{SocketOption, ErrorCode, SocketType};
use msg;
use msg::Msg;
use result::{ZmqError, ZmqResult};
use socket::ZmqSocket;
use socket_base::SocketBase;


enum State {
    Initial,
    Sending,
    Receiving,
}


pub struct ReqSocket {
    base: SocketBase,
    state: State,
    last_identity: uint,
    send_count: uint,
}


impl ZmqSocket for ReqSocket {
    fn getsockopt(&self, option: SocketOption) -> int {
        self.base.getsockopt(option)
    }

    fn bind(&self, addr: &str) -> ZmqResult<()> {
        self.base.bind(addr)
    }

    fn connect(&self, addr: &str) -> ZmqResult<()> {
        self.base.connect(addr)
    }

    fn msg_recv(&mut self) -> ZmqResult<Box<Msg>> {
        let ret = match self.state {
            State::Receiving => {
                self.base.recv_from(self.last_identity)
            },
            _ => return Err(ZmqError::new(
                ErrorCode::EFSM, "Operation cannot be accomplished in current state")),
        };
        self.state = match ret.flags & msg::MORE {
            0 => State::Initial,
            _ => State::Receiving,
        };
        Ok(ret)
    }

    fn msg_send(&mut self, msg: Box<Msg>) -> ZmqResult<()> {
        let flags = msg.flags;
        match self.state {
            State::Initial => {
                let (count, id) = self.base.round_robin(self.send_count);
                self.send_count = count;
                self.base.send_to(id, msg);
                self.last_identity = id;
            },
            State::Sending => {
                self.base.send_to(self.last_identity, msg);
            },
            _ => return Err(ZmqError::new(
                ErrorCode::EFSM, "Operation cannot be accomplished in current state")),
        }
        self.state = match flags & msg::MORE {
            0 => State::Receiving,
            _ => State::Sending,
        };
        Ok(())
    }
}


pub fn new(base: SocketBase) -> ReqSocket {
    base.set_type(SocketType::REQ);
    ReqSocket {
        base: base,
        state: State::Initial,
        last_identity: 0,
        send_count: 0,
    }
}


#[cfg(test)]
mod test {
    use ctx::Context;
    use consts::{SocketType, ErrorCode};

    #[test]
    fn test_fsm() {
        let ctx = Context::new();
        let mut s = ctx.socket(SocketType::REQ);
        assert_eq!(s.msg_recv().unwrap_err().code, ErrorCode::EFSM);
    }
}
