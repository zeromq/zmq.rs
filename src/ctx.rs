use consts;
use inproc::InprocManager;
use socket_base::SocketBase;
use rep::RepSocket;
use req::ReqSocket;


pub struct Context {
    inproc_mgr: InprocManager,
}

impl Context {
    pub fn new() -> Context {
        Context {
            inproc_mgr: InprocManager::new(),
        }
    }

    pub fn socket(&self, type_: consts::SocketType) -> Box<SocketBase + Send> {
        match type_ {
            consts::REQ => {
                let ret: ReqSocket = SocketBase::new(self.inproc_mgr.chan());
                box ret as Box<SocketBase + Send>
            },
            consts::REP => {
                let ret: RepSocket = SocketBase::new(self.inproc_mgr.chan());
                box ret as Box<SocketBase + Send>
            },
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
