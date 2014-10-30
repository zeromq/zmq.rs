use result::ZmqResult;
use socket_base::{SocketMessage, OnConnected};

use std::collections::HashMap;
use std::collections::hashmap::{Vacant, Occupied};


pub enum InprocCommand {
    DoBind(String, Sender<ZmqResult<SocketMessage>>),
    DoConnect(String, Sender<ZmqResult<SocketMessage>>),
}


struct InprocManagerTask {
    chan: Receiver<InprocCommand>,
    inproc_binders: HashMap<String, Sender<ZmqResult<SocketMessage>>>,
    inproc_connecters: HashMap<String, Vec<Sender<ZmqResult<SocketMessage>>>>,
}

impl InprocManagerTask {
    fn run(&mut self) {
        loop {
            match self.chan.recv_opt() {
                Ok(DoBind(key, tx)) => {
                    if self.inproc_binders.contains_key(&key) {
                        // TODO: return error
                        fail!("Key already exist: {}", key);
                    }

                    if self.inproc_connecters.contains_key(&key) {
                        let ref connecters = self.inproc_connecters[key];
                        for connecter_tx in connecters.iter() {
                            let (tx1, rx1) = channel();
                            let (tx2, rx2) = channel();
                            connecter_tx.send(Ok(OnConnected(tx1, rx2)));
                            tx.send(Ok(OnConnected(tx2, rx1)));
                        }
                    }

                    self.inproc_binders.insert(key.clone(), tx);
                },
                Ok(DoConnect(key, tx)) => {
                    if self.inproc_binders.contains_key(&key) {
                        let ref binder_tx = self.inproc_binders[key];
                        let (tx1, rx1) = channel();
                        let (tx2, rx2) = channel();
                        binder_tx.send(Ok(OnConnected(tx1, rx2)));
                        tx.send(Ok(OnConnected(tx2, rx1)));
                    }

                    match self.inproc_connecters.entry(key) {
                        Vacant(entry) => {
                            entry.set(vec!());
                        },
                        Occupied(entry) => {
                            entry.into_mut().push(tx);
                        }
                    }
                },
                _ => break,
            }
        }
    }
}


pub struct InprocManager {
    chan: Sender<InprocCommand>,
}

impl InprocManager {
    pub fn new() -> InprocManager {
        let (tx, rx) = channel();

        spawn(proc() {
            InprocManagerTask {
                chan: rx,
                inproc_binders: HashMap::new(),
                inproc_connecters: HashMap::new(),
            }.run();
        });

        InprocManager {
            chan: tx,
        }
    }

    pub fn chan(&self) -> Sender<InprocCommand> {
        self.chan.clone()
    }
}
