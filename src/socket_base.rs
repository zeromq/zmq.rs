use endpoint::Endpoint;

use std::collections::{HashMap, DList, Deque};
use std::comm::Select;
use std::io::net::tcp::TcpAcceptor;
use std::io::TcpStream;


pub enum SocketMessage {
    DoBind(TcpAcceptor),
    OnConnected(TcpStream),
}


pub struct SocketBase {
    endpoints: DList<Box<Endpoint>>,
}

impl SocketBase {
    pub fn new() -> SocketBase {
        SocketBase {
            endpoints: DList::new(),
        }
    }

    pub fn run(&mut self) {
        loop {
            // in order to call `handle` with a mutable `self`, we have to move the endpoints
            // TODO: this doesn't look cool, we need a better solution
            let mut endpoints = Vec::new();
            loop {
                match self.endpoints.pop_front() {
                    Some(endpoint) => {
                        endpoints.push(endpoint);
                    }
                    None => break
                }
            }
            let (msg, index) = {
                let selector = Select::new();
                let mut mapping = HashMap::new();
                let mut index = 0;
                for endpoint in endpoints.iter() {
                    let handle = box selector.handle(endpoint.get_chan());
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
                            Ok(msg) => (msg, index),
                            _ => break,
                        }
                    }
                    None => fail!(),
                }
            };
            endpoints.get_mut(index).handle(msg, self);
            self.endpoints.extend(endpoints.move_iter());
        }
    }

    pub fn add_endpoint(&mut self, endpoint: Box<Endpoint>) {
        self.endpoints.push_back(endpoint);
    }
}
