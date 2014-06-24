use consts;
use endpoint::Endpoint;
use msg::Msg;
use options::Options;
use result::{ZmqError, ZmqResult};
use socket_base::{DoBind, FeedChannel, SocketMessage};
use socket_base::SocketBase;
use tcp_listener::TcpListener;

use std::collections::HashMap;
use std::comm::Select;
use std::io;
use std::io::Listener;
use std::io::net::ip::SocketAddr;
use std::sync::{RWLock, Arc};


struct InnerZmqSocket {
    rx: Receiver<ZmqResult<SocketMessage>>,
}

impl Endpoint for InnerZmqSocket {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>> {
        &self.rx
    }

    fn in_event(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut SocketBase) {
        match msg {
            Ok(DoBind(acceptor)) => {
                socket.add_endpoint(box TcpListener::new(acceptor));
            }
            _ => ()
        }
    }

    fn is_critical(&self) -> bool {
        true
    }
}


pub struct ZmqSocket {
    tx: Sender<ZmqResult<SocketMessage>>,
    rx: Receiver<ZmqResult<SocketMessage>>,
    msg_channels: Vec<Receiver<Box<Msg>>>,
    options: Arc<RWLock<Options>>,
}

impl ZmqSocket {
    pub fn new(type_: consts::SocketType) -> ZmqSocket {
        let (tx, rx) = channel();
        let (out_tx, out_rx) = channel();
        let ret = ZmqSocket {
            tx: tx,
            rx: out_rx,
            msg_channels: Vec::new(),
            options: Arc::new(RWLock::new(Options::new())),
        };
        ret.options.write().type_ = type_ as int;
        let options_on_arc = ret.options.clone();
        spawn(proc() {
            let mut socket = SocketBase::new(options_on_arc, out_tx);
            let endpoint = box InnerZmqSocket {
                rx: rx,
            };
            socket.add_endpoint(endpoint);
            socket.run();
        });
        ret
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
                        self.tx.send(Ok(DoBind(acceptor)));
                        Ok(())
                    }
                    None => Err(ZmqError::new(
                        consts::EINVAL, "Invaid argument: bad address")),
                }},
            _ => Err(ZmqError::new(consts::EPROTONOSUPPORT, "Protocol not supported")),
        }
    }

    pub fn getsockopt(&self, option: consts::SocketOption) -> int {
        self.options.read().getsockopt(option)
    }

    pub fn msg_recv(&mut self) -> Box<Msg> {
        loop {
            let to_remove = {
                self.sync();
                let selector = Select::new();
                let mut mapping = HashMap::new();
                let mut index = 0;
                for chan in self.msg_channels.iter() {
                    let handle = box selector.handle(chan);
                    let hid = handle.id();
                    mapping.insert(hid, (handle, index));
                    let handle = mapping.get_mut(&hid).mut0();
                    unsafe {
                        handle.add();
                    }
                    index += 1;
                }
                let hid = selector.wait();
                match mapping.pop(&hid) {
                    Some((mut handle, index)) => {
                        match handle.recv_opt() {
                            Ok(msg) => return msg,
                            _ => index,
                        }
                    }
                    None => fail!(),
                }
            };
            self.msg_channels.remove(to_remove);
        }
    }

    fn sync(&mut self) {
        loop {
            if self.msg_channels.len() == 0 {
                match self.rx.recv_opt() {
                    Ok(msg) => self.handle_msg(msg),
                    Err(_) => continue,
                }
            }
            match self.rx.try_recv() {
                Ok(msg) => self.handle_msg(msg),
                Err(_) => break,
            }
        }
    }

    fn handle_msg(&mut self, msg: ZmqResult<SocketMessage>) {
        match msg {
            Ok(FeedChannel(chan)) => {
                self.msg_channels.push(chan);
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
