use msg::Msg;
use options::Options;
use result::ZmqResult;
use socket_base::{OnConnected, SocketMessage};

use std::collections::HashMap;
use std::comm::Select;
use std::sync::{RWLock, Arc};


struct Peer {
    sender: Sender<Box<Msg>>,
    receiver: Receiver<Box<Msg>>,
}

impl Peer {
    fn new(sender: Sender<Box<Msg>>, receiver: Receiver<Box<Msg>>) -> Peer {
        Peer {
            sender: sender,
            receiver: receiver,
        }
    }
}


pub struct PeerManager {
    pub options: Arc<RWLock<Options>>,
    pub tx: Sender<ZmqResult<SocketMessage>>,
    rx: Receiver<ZmqResult<SocketMessage>>,
    pub peers: HashMap<uint, Peer>,
    ids: Vec<uint>,
}

impl PeerManager {
    pub fn new() -> PeerManager {
        let (tx, rx) = channel();
        PeerManager {
            options: Arc::new(RWLock::new(Options::new())),
            tx: tx,
            rx: rx,
            peers: HashMap::new(),
            ids: Vec::new(),
        }
    }

    pub fn recv_first(&mut self) -> (uint, Box<Msg>) {
        loop {
            self.sync_until(|pm| { pm.peers.len() > 0 });
            let to_remove = {
                let selector = Select::new();
                let mut mapping = HashMap::new();
                let mut index = 0;
                for id in self.ids.iter() {
                    let peer = self.peers.get(id);
                    let handle = box selector.handle(&peer.receiver);
                    let hid = handle.id();
                    mapping.insert(hid, (Some(handle), index));
                    let handle = mapping.get_mut(&hid).mut0().get_mut_ref();
                    unsafe {
                        handle.add();
                    }
                    index += 1;
                }
                let handle = box selector.handle(&self.rx);
                let hid = handle.id();
                mapping.insert(hid, (None, 0));
                let hid = selector.wait();
                match mapping.pop(&hid) {
                    Some((None, _)) => continue,
                    Some((Some(mut handle), index)) => {
                        match handle.recv_opt() {
                            Ok(msg) => return (*self.ids.get(index), msg),
                            _ => index,
                        }
                    },
                    None => fail!(),
                }
            };
            match self.ids.remove(to_remove) {
                Some(id) => {
                    self.peers.remove(&id);
                },
                None => (),
            }
        }
    }

    pub fn recv_from(&mut self, id: uint) -> Box<Msg> {
        self.sync_until(|pm| { pm.peers.contains_key(&id) });
        self.peers.get(&id).receiver.recv()
    }

    pub fn send_to(&mut self, id: uint, msg: Box<Msg>) {
        debug!("Sending {} to {}", msg, id);
        self.sync_until(|pm| { pm.peers.contains_key(&id) });
        self.peers.get(&id).sender.send(msg);
    }

    pub fn round_robin(&mut self, mut index: uint) -> (uint, uint) {
        self.sync_until(|pm| { pm.ids.len() > 0 });
        index = (index + 1) % self.ids.len();
        (index, *self.ids.get(index))
    }

    fn sync_until(&mut self, cond: |&PeerManager| -> bool) {
        loop {
            if !cond(self) {
                debug!("Condition not met, wait... peers: {}", self.peers.len());
                match self.rx.recv_opt() {
                    Ok(msg) => self.handle_msg(msg),
                    Err(_) => fail!(),
                }
                continue;
            }
            match self.rx.try_recv() {
                Ok(msg) => self.handle_msg(msg),
                Err(_) => break,
            }
        }
    }

    fn handle_msg(&mut self, msg: ZmqResult<SocketMessage>) {
        match msg {
            Ok(OnConnected(tx, rx)) => {
                let id = self.ids.last().unwrap_or(&0) + 1;
                debug!("New peer: {}", id);
                self.ids.push(id);
                self.peers.insert(id, Peer::new(tx, rx));
            }
            _ => (),
        }
    }
}
