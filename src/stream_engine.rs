use endpoint::Endpoint;
use options::Options;
use result::ZmqResult;
use socket_base::{SocketBase, SocketMessage};

use std::io::TcpStream;
use std::sync::{RWLock, Arc};


struct InnerStreamEngine {
    chan: Sender<ZmqResult<SocketMessage>>,
    stream: TcpStream,
    options: Arc<RWLock<Options>>,
}

impl InnerStreamEngine {
    fn run(&mut self) {
    }
}


pub struct StreamEngine {
    chan: Receiver<ZmqResult<SocketMessage>>,
    stream: TcpStream,
}

impl StreamEngine {
    pub fn new(stream: TcpStream, options: Arc<RWLock<Options>>) -> StreamEngine {
        let (tx, rx) = channel();
        let receiver = stream.clone();
        spawn(proc() {
            let mut engine = InnerStreamEngine {
                chan: tx,
                stream: receiver,
                options: options,
            };
            engine.run();
        });

        StreamEngine {
            chan: rx,
            stream: stream,
        }
    }
}

impl Endpoint for StreamEngine {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>> {
        &self.chan
    }

    fn in_event(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut SocketBase) {
    }

    fn is_critical(&self) -> bool {
        false
    }
}
