use consts;
use inproc::{InprocCommand, DoBind, DoConnect};
use msg::Msg;
use options::Options;
use result::{ZmqError, ZmqResult};
use tcp_connecter::TcpConnecter;
use tcp_listener::TcpListener;

use std::collections::HashMap;
use std::comm::Select;
use std::io;
use std::io::Listener;
use std::io::net::ip::SocketAddr;
use std::sync::{Arc, RWLock};


pub enum SocketMessage {
    Ping,
    OnConnected(Sender<Box<Msg>>, Receiver<Box<Msg>>),
}


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


pub struct SocketBase {
    options: Arc<RWLock<Options>>,
    tx: Sender<ZmqResult<SocketMessage>>,
    rx: Receiver<ZmqResult<SocketMessage>>,
    peers: HashMap<uint, Peer>,
    ids: Vec<uint>,
    inproc_chan: Sender<InprocCommand>,
}

impl SocketBase {
    pub fn new(chan: Sender<InprocCommand>) -> SocketBase {
        let (tx, rx) = channel();
        SocketBase {
            options: Arc::new(RWLock::new(Options::new())),
            tx: tx,
            rx: rx,
            peers: HashMap::new(),
            ids: Vec::new(),
            inproc_chan: chan,
        }
    }

    pub fn set_type(&self, type_: consts::SocketType) {
        self.options.write().type_ = type_ as int;
    }

    pub fn bind(&self, addr: &str) -> ZmqResult<()> {
        let (protocol, address) = try!(parse_uri(addr));

        match protocol {
            "tcp" => {
                match from_str::<SocketAddr>(address) {
                    Some(addr) => {
                        let listener = io::TcpListener::bind(
                            format!("{}", addr.ip).as_slice(), addr.port);
                        let acceptor = try!(listener.listen().map_err(ZmqError::from_io_error));
                        TcpListener::spawn_new(acceptor, self.tx.clone(), self.options.clone());
                        Ok(())
                    }
                    None => Err(ZmqError::new(
                        consts::EINVAL, "Invaid argument: bad address")),
                }
            },
            "inproc" => {
                self.inproc_chan.send(DoBind(String::from_str(address), self.tx.clone()));
                Ok(())
            },
            _ => Err(ZmqError::new(consts::EPROTONOSUPPORT, "Protocol not supported")),
        }
    }

    pub fn connect(&self, addr: &str) -> ZmqResult<()> {
        let (protocol, address) = try!(parse_uri(addr));
        match protocol {
            "tcp" => {
                match from_str::<SocketAddr>(address) {
                    Some(addr) => {
                        TcpConnecter::spawn_new(addr, self.tx.clone(), self.options.clone());
                        Ok(())
                    }
                    None => Err(ZmqError::new(
                        consts::EINVAL, "Invaid argument: bad address")),
                }
            },
            "inproc" => {
                self.inproc_chan.send(DoConnect(String::from_str(address), self.tx.clone()));
                Ok(())
            },
            _ => Err(ZmqError::new(consts::EPROTONOSUPPORT, "Protocol not supported")),
        }
    }

    pub fn getsockopt(&self, option: consts::SocketOption) -> int {
        self.options.read().getsockopt(option)
    }

    pub fn recv_first(&mut self) -> (uint, Box<Msg>) {
        loop {
            self.sync_until(|s| { s.peers.len() > 0 });
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
                            Ok(msg) => {
                                unsafe {
                                    handle.remove();
                                }
                                return (*self.ids.get(index), msg);
                            }
                            _ => {
                                unsafe {
                                    handle.remove();
                                }
                                index
                            }
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
        self.sync_until(|s| { s.peers.contains_key(&id) });
        self.peers.get(&id).receiver.recv()
    }

    pub fn send_to(&mut self, id: uint, msg: Box<Msg>) {
        debug!("Sending {} to {}", msg, id);
        self.sync_until(|s| { s.peers.contains_key(&id) });
        self.peers.get(&id).sender.send(msg);
    }

    pub fn round_robin(&mut self, mut index: uint) -> (uint, uint) {
        self.sync_until(|s| { s.ids.len() > 0 });
        index = (index + 1) % self.ids.len();
        (index, *self.ids.get(index))
    }

    fn sync_until(&mut self, cond: |&SocketBase| -> bool) {
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


fn parse_uri<'r>(uri: &'r str) -> ZmqResult<(&'r str, &'r str)> {
    match uri.find_str("://") {
        Some(pos) => {
            let protocol = uri.slice_to(pos);
            let address = uri.slice_from(pos + 3);
            if protocol.len() == 0 || address.len() == 0 {
                Err(ZmqError::new(
                    consts::EINVAL,
                    "Invalid argument: missing protocol or address"))
            } else {
                Ok((protocol, address))
            }
        },
        None => Err(ZmqError::new(
            consts::EINVAL, "Invalid argument: missing ://")),
    }
}


#[cfg(test)]
mod test {
    use super::parse_uri;

    #[test]
    fn test_parse_uri() {
        assert!(parse_uri("").is_err());
        assert!(parse_uri("://").is_err());
        assert!(parse_uri("tcp://").is_err());
        assert!(parse_uri("://127.0.0.1").is_err());
        match parse_uri("tcp://127.0.0.1:8890") {
            Ok((protocol, address)) => {
                assert_eq!(protocol, "tcp");
                assert_eq!(address, "127.0.0.1:8890");
            },
            Err(_) => {assert!(false);},
        }
    }
}
