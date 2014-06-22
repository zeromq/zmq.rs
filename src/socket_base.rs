use endpoint::Endpoint;
use options::Options;

use std::collections::{HashMap, DList, Deque};
use std::comm::Select;
use std::io::net::tcp::TcpAcceptor;
use std::io::TcpStream;
use std::sync::{RWLock, Arc};


pub enum SocketMessage {
    DoBind(TcpAcceptor),
    OnConnected(TcpStream),
}


pub struct SocketBase {
    endpoints: DList<Box<Endpoint>>,
    options: Arc<RWLock<Options>>,
}

impl SocketBase {
    pub fn new(options: Arc<RWLock<Options>>) -> SocketBase {
        SocketBase {
            endpoints: DList::new(),
            options: options,
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
                            Ok(msg) => (Some(msg), index),
                            _ => (None, index),
                        }
                    }
                    None => fail!(),
                }
            };
            match msg {
                Some(msg) => endpoints.get_mut(index).in_event(msg, self),
                None if endpoints.get(index).is_critical() => break,
                _ => { endpoints.remove(index); }
            }
            self.endpoints.extend(endpoints.move_iter());
        }
    }

    pub fn add_endpoint(&mut self, endpoint: Box<Endpoint>) {
        self.endpoints.push_back(endpoint);
    }

    pub fn clone_options(&self) -> Arc<RWLock<Options>> {
        self.options.clone()
    }
}
