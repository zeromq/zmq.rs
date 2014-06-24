use endpoint::Endpoint;
use options::Options;
use result::ZmqResult;
use socket_base::{FeedChannel, SocketBase, SocketMessage};
use v2_decoder::V2Decoder;

use std::io::extensions;
use std::io::{TcpStream, Reader};
use std::sync::{RWLock, Arc};


static V2_GREETING_SIZE: uint = 12;
static NO_PROGRESS_LIMIT: uint = 1000;
static SIGNATURE_SIZE: uint = 10;
static ZMTP_1_0: u8 = 0;
static ZMTP_2_0: u8 = 1;
static REVISION_POS: uint = 10;


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
    chan_to_socket: Sender<ZmqResult<SocketMessage>>,
    stream: TcpStream,
    options: Arc<RWLock<Options>>,
    sender: Sender<Box<Vec<u8>>>,
    decoder: Option<V2Decoder>,
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

        self.handshake();

        assert!(self.decoder.is_some());
        let (tx, rx) = channel(); // TODO: replace with SyncSender
        self.chan_to_socket.send(Ok(FeedChannel(rx))) ;

        loop {
            match self.decoder.get_mut_ref().decode(&mut self.stream) {
                Ok(msg) => tx.send(msg),
                Err(e) => {
                    self.chan_to_socket.send(Err(e));
                    break;
                }
            }
        }
    }

    fn handshake(&mut self) {
        let mut greeting_recv = [0u8, ..V2_GREETING_SIZE];
        let mut greeting_bytes_read = 0;
        let mut zeros = 0;
        let mut type_sent = false;

        while greeting_bytes_read < V2_GREETING_SIZE {
            match self.stream.read(greeting_recv.mut_slice_from(greeting_bytes_read)) {
                Ok(0) => {
                    println!("ZERO");
                    zeros += 1;
                    if zeros > NO_PROGRESS_LIMIT {
                        return; // TODO: error
                    } else {
                        continue;
                    }
                }
                Ok(n) => {
                    greeting_bytes_read += n;
                }
                _ => return, // TODO: error
            }
            println!(">>> Greeting bytes so far: {}", greeting_recv.as_slice());

            //  We have received at least one byte from the peer.
            //  If the first byte is not 0xff, we know that the
            //  peer is using unversioned protocol.
            if greeting_recv[0] != 0xff {
                return; // TODO: error or ZMTP 1.0
            }

            if greeting_bytes_read < SIGNATURE_SIZE {
                continue;
            }

            //  Inspect the right-most bit of the 10th byte (which coincides
            //  with the 'flags' field if a regular message was sent).
            //  Zero indicates this is a header of identity message
            //  (i.e. the peer is using the unversioned protocol).
            if greeting_recv[9] & 0x01 == 0 {
                return; // TODO: error or ZMTP 1.0
            }

            //  The peer is using versioned protocol.
            //  Send the version number.
            self.sender.send(box [1u8].into_owned());

            if greeting_bytes_read > SIGNATURE_SIZE && !type_sent {
                type_sent = true;
                match greeting_recv[10] {
                    ZMTP_1_0 | ZMTP_2_0 => {
                        println!(">>> ZMTP 2.0");
                        self.sender.send(box [self.options.read().type_ as u8].into_owned());
                    }
                    _ => {
                        println!(">>> ZMTP 3.0");
                        //TODO: error or ZMTP 3.0
                        self.sender.send(box [self.options.read().type_ as u8].into_owned());
                    }
                }
            }
        }

        if greeting_recv[0] != 0xff || greeting_recv[9] & 0x01 == 0 {
            // TODO: error or ZMTP 1.0
        } else if greeting_recv[REVISION_POS] == ZMTP_1_0 {
            // TODO: error or ZMTP 1.0
        } else {
            assert!(self.decoder.is_none());
            self.decoder = Some(V2Decoder::new(self.options.read().maxmsgsize));
            //TODO: set encoder
        }
    }
}


pub struct StreamEngine {
    // the normal channel for SocketMessage
    chan_from_inner: Receiver<ZmqResult<SocketMessage>>,

    // a send handle
    sender: Sender<Box<Vec<u8>>>,
}

impl StreamEngine {
    pub fn new(stream: TcpStream, options: Arc<RWLock<Options>>) -> StreamEngine {
        // the normal channel for SocketMessage
        let (chan_to_socket, chan_from_inner) = channel();

        // two handles of one TcpStream: send and recv
        let receiver = stream.clone();

        // the channel for sending data
        let (stx, srx) = channel();
        let stx2 = stx.clone();

        spawn(proc() {
            let mut engine = InnerStreamEngine {
                chan_to_socket: chan_to_socket,
                stream: receiver,
                options: options,
                sender: stx2,
                decoder: None,
            };
            engine.run();
            engine.stream.close_read();
            engine.stream.close_write();
        });
        spawn(proc() {
            proxy_write(srx, stream);
        });

        StreamEngine {
            chan_from_inner: chan_from_inner,
            sender: stx,
        }
    }
}

impl Endpoint for StreamEngine {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>> {
        &self.chan_from_inner
    }

    fn in_event(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut SocketBase) {
        match msg {
            Ok(FeedChannel(rx)) => socket.send_back(Ok(FeedChannel(rx))),
            _ => ()
        }
    }
}
