use consts;
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
    fn getsockopt(&self, option: consts::SocketOption) -> int {
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
            Initial => self.base.recv_first(),
            Receiving => (self.last_identity, self.base.recv_from(self.last_identity)),
            _ => return Err(ZmqError::new(
                consts::EFSM, "Operation cannot be accomplished in current state")),
        };
        self.last_identity = id;
        self.state = match ret.flags & msg::MORE {
            0 => Sending,
            _ => Receiving,
        };
        Ok(ret)
    }

    fn msg_send(&mut self, msg: Box<Msg>) -> ZmqResult<()> {
        self.state = match self.state {
            Sending => {
                let flags = msg.flags;
                self.base.send_to(self.last_identity, msg);
                match flags & msg::MORE {
                    0 => Initial,
                    _ => Sending,
                }
            }
            _ => return Err(ZmqError::new(
                consts::EFSM, "Operation cannot be accomplished in current state")),
        };
        Ok(())
    }
}


pub fn new(base: SocketBase) -> RepSocket {
    base.set_type(consts::REP);
    RepSocket {
        base: base,
        state: Initial,
        last_identity: 0,
    }
}


#[cfg(test)]
mod test {
    use ctx::Context;
    use consts;
    use msg::Msg;

    #[test]
    fn test_fsm() {
        let ctx = Context::new();
        let mut s = ctx.socket(consts::REP);
        let msg = box Msg::new(1);
        assert_eq!(s.msg_send(msg).unwrap_err().code, consts::EFSM);
    }
}
