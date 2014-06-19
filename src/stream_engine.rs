use endpoint::Endpoint;
use result::ZmqResult;
use socket_base::{SocketBase, SocketMessage};

use std::io::TcpStream;


struct InnerStreamEngine {
    chan: Sender<ZmqResult<SocketMessage>>,
    stream: TcpStream,
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
    pub fn new(stream: TcpStream) -> StreamEngine {
        let (tx, rx) = channel();
        let receiver = stream.clone();
        spawn(proc() {
            let mut engine = InnerStreamEngine {
                chan: tx,
                stream: receiver,
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
