use consts;
use msg;
use msg::Msg;
use peer::PeerManager;
use result::{ZmqError, ZmqResult};
use socket_base::SocketBase;


enum State {
    Initial,
    Receiving,
    Sending,
}


pub struct RepSocket {
    pm: PeerManager,
    state: State,
    last_identity: uint,
}

impl SocketBase for RepSocket {
    fn new() -> RepSocket {
        RepSocket {
            pm: PeerManager::new(),
            state: Initial,
            last_identity: 0,
        }.init(consts::REP)
    }

    fn pm<'a>(&'a self) -> &'a PeerManager {
        &self.pm
    }

    fn pmut<'a>(&'a mut self) -> &'a mut PeerManager {
        &mut self.pm
    }

    fn msg_recv(&mut self) -> ZmqResult<Box<Msg>> {
        let last_identity = self.last_identity;
        let (id, ret) = match self.state {
            Initial => self.pmut().recv_first(),
            Receiving => (last_identity, self.pmut().recv_from(last_identity)),
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
                let id = self.last_identity;
                let flags = msg.flags;
                self.pmut().send_to(id, msg);
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
