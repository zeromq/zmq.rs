use consts;
use inproc::InprocManager;
use rep;
use req;
use socket::ZmqSocket;
use socket_base::SocketBase;


pub struct Context {
    inproc_mgr: InprocManager,
}

impl Context {
    pub fn new() -> Context {
        Context {
            inproc_mgr: InprocManager::new(),
        }
    }

    pub fn socket(&self, type_: consts::SocketType) -> Box<ZmqSocket + Send> {
        let base = SocketBase::new(self.inproc_mgr.chan());
        match type_ {
            consts::REQ => box req::new(base) as Box<ZmqSocket + Send>,
            consts::REP => box rep::new(base) as Box<ZmqSocket + Send>,
        }
    }
}


#[cfg(test)]
mod test {
    use ctx::Context;

    #[test]
    fn test_new() {
        Context::new();
    }
}
