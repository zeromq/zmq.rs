use consts;
use socket_base::{SocketBase, SocketMessage, OnConnected};
use rep::RepSocket;
use req::ReqSocket;
use result::ZmqResult;

use std::collections::HashMap;


pub struct Context {
    inproc_binders: HashMap<String, Sender<ZmqResult<SocketMessage>>>,
    inproc_connecters: HashMap<String, Vec<Sender<ZmqResult<SocketMessage>>>>,
}

impl Context {
    pub fn new() -> Context {
        Context {
            inproc_binders: HashMap::new(),
            inproc_connecters: HashMap::new(),
        }
    }

    pub fn socket(&mut self, type_: consts::SocketType) -> Box<SocketBase> {
        match type_ {
            consts::REQ => {
                let ret: ReqSocket = SocketBase::new(self);
                box ret as Box<SocketBase>
            },
            consts::REP => {
                let ret: RepSocket = SocketBase::new(self);
                box ret as Box<SocketBase>
            },
        }
    }

    pub fn _bind_inproc(&mut self, address: &str, tx: Sender<ZmqResult<SocketMessage>>) {
        let key = String::from_str(address);

        if self.inproc_binders.contains_key(&key) {
            // TODO: return error
            fail!();
        }

        if self.inproc_connecters.contains_key(&key) {
            let connecters = self.inproc_connecters.get(&key);
            for connecter_tx in connecters.iter() {
                let (tx1, rx1) = channel();
                let (tx2, rx2) = channel();
                connecter_tx.send(Ok(OnConnected(tx1, rx2)));
                tx.send(Ok(OnConnected(tx2, rx1)));
            }
        }

        self.inproc_binders.insert(key.clone(), tx);
    }

    pub fn _connect_inproc(&mut self, address: &str, tx: Sender<ZmqResult<SocketMessage>>) {
        let key = String::from_str(address);

        if self.inproc_binders.contains_key(&key) {
            let binder_tx = self.inproc_binders.get(&key);
            let (tx1, rx1) = channel();
            let (tx2, rx2) = channel();
            binder_tx.send(Ok(OnConnected(tx1, rx2)));
            tx.send(Ok(OnConnected(tx2, rx1)));
        }

        self.inproc_connecters.find_or_insert(key.clone(), vec!()).push(tx);
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
