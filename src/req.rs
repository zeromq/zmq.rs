use consts;
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
        let ret = match self.state {
            Receiving => {
                self.base.recv_from(self.last_identity)
            },
            _ => return Err(ZmqError::new(
                consts::EFSM, "Operation cannot be accomplished in current state")),
        };
        self.state = match ret.flags & msg::MORE {
            0 => Initial,
            _ => Receiving,
        };
        Ok(ret)
    }

    fn msg_send(&mut self, msg: Box<Msg>) -> ZmqResult<()> {
        let flags = msg.flags;
        match self.state {
            Initial => {
                let (count, id) = self.base.round_robin(self.send_count);
                self.send_count = count;
                self.base.send_to(id, msg);
                self.last_identity = id;
            },
            Sending => {
                self.base.send_to(self.last_identity, msg);
            },
            _ => return Err(ZmqError::new(
                consts::EFSM, "Operation cannot be accomplished in current state")),
        }
        self.state = match flags & msg::MORE {
            0 => Receiving,
            _ => Sending,
        };
        Ok(())
    }
}


pub fn new(base: SocketBase) -> ReqSocket {
    base.set_type(consts::REQ);
    ReqSocket {
        base: base,
        state: Initial,
        last_identity: 0,
        send_count: 0,
    }
}


#[cfg(test)]
mod test {
    use ctx::Context;
    use consts;

    #[test]
    fn test_fsm() {
        let ctx = Context::new();
        let mut s = ctx.socket(consts::REQ);
        assert_eq!(s.msg_recv().unwrap_err().code, consts::EFSM);
    }
}
