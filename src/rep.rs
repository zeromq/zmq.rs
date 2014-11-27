use consts::{SocketOption, ErrorCode, SocketType};
use msg;
use msg::Msg;
use result::{ZmqError, ZmqResult};
use socket::ZmqSocket;
use socket_base::SocketBase;


enum State {
    Initial,
    Receiving,
    Sending,
}


pub struct RepSocket {
    base: SocketBase,
    state: State,
    last_identity: uint,
}


impl ZmqSocket for RepSocket {
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
        let (id, ret) = match self.state {
            State::Initial => self.base.recv_first(),
            State::Receiving => (self.last_identity, self.base.recv_from(self.last_identity)),
            _ => return Err(ZmqError::new(
                ErrorCode::EFSM, "Operation cannot be accomplished in current state")),
        };
        self.last_identity = id;
        self.state = match ret.flags & msg::MORE {
            0 => State::Sending,
            _ => State::Receiving,
        };
        Ok(ret)
    }

    fn msg_send(&mut self, msg: Box<Msg>) -> ZmqResult<()> {
        self.state = match self.state {
            State::Sending => {
                let flags = msg.flags;
                self.base.send_to(self.last_identity, msg);
                match flags & msg::MORE {
                    0 => State::Initial,
                    _ => State::Sending,
                }
            }
            _ => return Err(ZmqError::new(
                ErrorCode::EFSM, "Operation cannot be accomplished in current state")),
        };
        Ok(())
    }
}


pub fn new(base: SocketBase) -> RepSocket {
    base.set_type(SocketType::REP);
    RepSocket {
        base: base,
        state: State::Initial,
        last_identity: 0,
    }
}


#[cfg(test)]
mod test {
    use ctx::Context;
    use consts::{ErrorCode, SocketType};
    use msg::Msg;

    #[test]
    fn test_fsm() {
        let ctx = Context::new();
        let mut s = ctx.socket(SocketType::REP);
        let msg = box Msg::new(1);
        assert_eq!(s.msg_send(msg).unwrap_err().code, ErrorCode::EFSM);
    }
}
