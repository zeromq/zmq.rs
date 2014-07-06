use consts;
use inproc::InprocCommand;
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
    fn new(chan: Sender<InprocCommand>) -> ReqSocket {
        ReqSocket {
            pm: PeerManager::new(chan),
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
                self.pm.recv_from(self.last_identity)
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
                let (count, id) = self.pm.round_robin(self.send_count);
                self.send_count = count;
                self.pm.send_to(id, msg);
                self.last_identity = id;
            },
            Sending => {
                self.pm.send_to(self.last_identity, msg);
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
