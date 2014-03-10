use consts;
use socket_base::SocketBase;
use req::ReqSocket;


pub struct Context {
    starting: bool,
    terminating: bool,
}

impl Context {
    pub fn new() -> Context {
        Context {
            starting: true,
            terminating: false,
        }
    }

    pub fn socket(&self, type_: consts::SocketType) -> ~SocketBase {
        match type_ {
            consts::REQ => {
                let ret: ReqSocket = SocketBase::create();
                ~ret as ~SocketBase
            },
        }
    }
}


#[cfg(test)]
mod test {
    use ctx::Context;

    #[test]
    fn test_new() {
        let ctx = Context::new();
        assert_eq!(ctx.starting, true);
    }
}

