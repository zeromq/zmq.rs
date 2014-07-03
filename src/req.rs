use consts;
use msg;
use msg::Msg;
use peer::PeerManager;
use result::{ZmqError, ZmqResult};
use socket_base::SocketBase;


enum State {
    Initial,
    Sending,
    Receiving,
}


pub struct ReqSocket {
    pm: PeerManager,
    state: State,
    last_identity: uint,
    send_count: uint,
}

impl SocketBase for ReqSocket {
    fn new() -> ReqSocket {
        ReqSocket {
            pm: PeerManager::new(),
            state: Initial,
            last_identity: 0,
            send_count: 0,
        }.init(consts::REQ)
    }

    fn pm<'a>(&'a self) -> &'a PeerManager {
        &self.pm
    }

    fn pmut<'a>(&'a mut self) -> &'a mut PeerManager {
        &mut self.pm
    }

    fn msg_recv(&mut self) -> ZmqResult<Box<Msg>> {
        let ret = match self.state {
            Receiving => {
                let id = self.last_identity;
                self.pmut().recv_from(id)
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
                let count = self.send_count;
                let (count, id) = self.pmut().round_robin(count);
                self.send_count = count;
                self.pmut().send_to(id, msg);
                self.last_identity = id;
            },
            Sending => {
                let id = self.last_identity;
                self.pmut().send_to(id, msg);
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
