use endpoint::Endpoint;
use options::Options;
use result::ZmqResult;
use socket_base::{SocketBase, SocketMessage};

use std::io::extensions;
use std::io::TcpStream;
use std::sync::{RWLock, Arc};


static V2_GREETING_SIZE: uint = 12;
static NO_PROGRESS_LIMIT: uint = 1000;
static SIGNATURE_SIZE: uint = 10;
static ZMTP_1_0: u8 = 0;
static ZMTP_2_0: u8 = 1;


fn proxy_write(chan: Receiver<Box<Vec<u8>>>, mut stream: TcpStream) {
    loop {
        match chan.recv_opt() {
            Ok(buf) => match stream.write(buf.as_slice()) {
                Err(_) => break,
                _ => ()
            },
            _ => break
        }
    }
}


struct InnerStreamEngine {
    chan: Sender<ZmqResult<SocketMessage>>,
    stream: TcpStream,
    options: Arc<RWLock<Options>>,
    sender: Sender<Box<Vec<u8>>>,
}

impl InnerStreamEngine {
    fn run(&mut self) {
        let mut signature = box vec!();
        signature.push(0xffu8);
        extensions::u64_to_be_bytes(
            (self.options.read().identity_size + 1) as u64, 8, |v| signature.push_all(v));
        signature.push(0x7fu8);
        println!(">>> Sending signature: {}", signature);
        self.sender.send(signature);

        let mut greeting_recv = [0u8, ..V2_GREETING_SIZE];
        let mut greeting_bytes_read = 0;
        let mut zeros = 0;
        while greeting_bytes_read < V2_GREETING_SIZE {
            match self.stream.read(greeting_recv.mut_slice_from(greeting_bytes_read)) {
                Ok(0) => {
                    println!("ZERO");
                    zeros += 1;
                    if zeros > NO_PROGRESS_LIMIT {
                        return;
                    } else {
                        continue;
                    }
                }
                Ok(n) => {
                    greeting_bytes_read += n;
                }
                _ => return,
            }
            println!(">>> Greeting bytes so far: {}", greeting_recv.as_slice());

            //  We have received at least one byte from the peer.
            //  If the first byte is not 0xff, we know that the
            //  peer is using unversioned protocol.
            if greeting_recv[0] != 0xff {
                return;
            }

            if greeting_bytes_read < SIGNATURE_SIZE {
                continue;
            }

            //  Inspect the right-most bit of the 10th byte (which coincides
            //  with the 'flags' field if a regular message was sent).
            //  Zero indicates this is a header of identity message
            //  (i.e. the peer is using the unversioned protocol).
            if greeting_recv[9] & 0x01 == 0 {
                return;
            }

            //  The peer is using versioned protocol.
            //  Send the version number.
            self.sender.send(box [1u8].into_owned());

            if greeting_bytes_read > SIGNATURE_SIZE {
                match greeting_recv[10] {
                    ZMTP_1_0 | ZMTP_2_0 => {
                        println!(">>> ZMTP 2.0");
                        // TODO: send options.type
                    }
                    _ => {
                        println!(">>> ZMTP 3.0");
                    }
                }
            }
        }

        loop {
            println!(">>> {}", self.stream.read_exact(1));
        }
    }
}


pub struct StreamEngine {
    chan: Receiver<ZmqResult<SocketMessage>>,
    sender: Sender<Box<Vec<u8>>>,
}

impl StreamEngine {
    pub fn new(stream: TcpStream, options: Arc<RWLock<Options>>) -> StreamEngine {
        let (tx, rx) = channel();
        let receiver = stream.clone();
        let (stx, srx) = channel();
        let stx2 = stx.clone();
        spawn(proc() {
            let mut engine = InnerStreamEngine {
                chan: tx,
                stream: receiver,
                options: options,
                sender: stx2,
            };
            engine.run();
            engine.stream.close_read();
            engine.stream.close_write();
        });
        spawn(proc() {
            proxy_write(srx, stream);
        });

        StreamEngine {
            chan: rx,
            sender: stx,
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
