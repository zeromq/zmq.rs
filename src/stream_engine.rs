use consts;
use msg::Msg;
use options::Options;
use result::{ZmqError, ZmqResult};
use socket_base::{OnConnected, SocketMessage};
use v2_encoder::V2Encoder;
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


fn stream_bytes_writer(chan: Receiver<Box<Vec<u8>>>, mut stream: TcpStream, _waiter: Sender<u8>) {
    loop {
        match chan.recv_opt() {
            Ok(buf) => match stream.write(buf.as_slice()) {
                Err(_) => break,
                _ => (),
            },
            _ => break
        }
    }
}


fn stream_msg_writer(msg_chan: Receiver<Box<Msg>>, mut stream: TcpStream, encoder: V2Encoder) {
    loop {
        match msg_chan.recv_opt() {
            Ok(msg) => {
                debug!("Sending message: {} @ {} -> {}", msg,
                       stream.socket_name(), stream.peer_name());
                match encoder.encode(msg, &mut stream) {
                    Err(_) => break,
                    _ => (),
                }
            },
            _ => break
        }
    }
    let _ = stream.close_read();
}


pub struct StreamEngine {
    chan_to_socket: Sender<ZmqResult<SocketMessage>>,
    stream: TcpStream,
    options: Arc<RWLock<Options>>,

    // a sender to unblock all receivers on drop
    _death_notifier: Option<Sender<u8>>,
}

impl StreamEngine {
    fn run(&mut self) -> ZmqResult<()> {
        info!("Connection is made: {} -> {}", self.stream.socket_name(), self.stream.peer_name());

        // prepare task for bufferring outgoing bytes
        let (bytes_tx, bytes_rx) = channel();
        let (waiter_tx, waiter_rx) = channel();
        let stream = self.stream.clone();
        spawn(proc() {
            stream_bytes_writer(bytes_rx, stream, waiter_tx);
        });

        // send signature
        let mut signature = box vec!();
        signature.push(0xffu8);
        extensions::u64_to_be_bytes(
            (self.options.read().identity_size + 1) as u64, 8, |v| signature.push_all(v));
        signature.push(0x7fu8);
        if bytes_tx.send_opt(signature).is_err() {
            return Err(ZmqError::new(consts::EIOERROR, "Connection closed"));
        }

        let (decoder, encoder) = try!(self.handshake(bytes_tx));
        let _ = waiter_rx.recv_opt(); // wait until all outgoing handshake bytes are committed
        debug!("Handshake is done: {} -> {}", self.stream.socket_name(), self.stream.peer_name());

        // prepare task for sending Msg objects
        let (msg_tx, msg_rx) = channel(); // TODO: replace with SyncSender
        let stream = self.stream.clone();
        spawn(proc() {
            stream_msg_writer(msg_rx, stream, encoder);
        });

        // Receive Msg objects
        let (tx, rx) = channel(); // TODO: replace with SyncSender
        debug!("Feeding the peer channels to the socket object.");
        if self.chan_to_socket.send_opt(Ok(OnConnected(msg_tx, rx))).is_err() {
            warn!("Socket object is gone!");
            return Ok(());
        }
        loop {
            match decoder.decode(&mut self.stream) {
                Ok(msg) => if tx.send_opt(msg).is_err() {
                    return Ok(());
                },
                Err(e) => {
                    let _ = self.chan_to_socket.send_opt(Err(e));
                    break;
                }
            }
        }

        Ok(())
    }

    fn handshake(&mut self, sender: Sender<Box<Vec<u8>>>) -> ZmqResult<(V2Decoder, V2Encoder)> {
        let mut greeting_recv = [0u8, ..V2_GREETING_SIZE];
        let mut greeting_bytes_read = 0;
        let mut zeros = 0;
        let mut type_sent = false;
        let mut version_sent = false;

        while greeting_bytes_read < V2_GREETING_SIZE {
            match self.stream.read(greeting_recv.mut_slice_from(greeting_bytes_read)) {
                Ok(0) => {
                    zeros += 1;
                    if zeros > NO_PROGRESS_LIMIT {
                        return Err(ZmqError::new(consts::EIOERROR, "No progress in handshake"));
                    } else {
                        continue;
                    }
                }
                Ok(n) => {
                    greeting_bytes_read += n;
                }
                Err(e) => return Err(ZmqError::from_io_error(e)),
            }

            //  We have received at least one byte from the peer.
            //  If the first byte is not 0xff, we know that the
            //  peer is using unversioned protocol.
            if greeting_recv[0] != 0xff {
                return Err(ZmqError::new(consts::EPROTONOSUPPORT, "ZMTP 1.0 is not supported"));
            }

            if greeting_bytes_read < SIGNATURE_SIZE {
                continue;
            }

            //  Inspect the right-most bit of the 10th byte (which coincides
            //  with the 'flags' field if a regular message was sent).
            //  Zero indicates this is a header of identity message
            //  (i.e. the peer is using the unversioned protocol).
            if greeting_recv[9] & 0x01 == 0 {
                return Err(ZmqError::new(consts::EPROTONOSUPPORT, "ZMTP 1.0 is not supported"));
            }

            //  The peer is using versioned protocol.
            //  Send the version number.
            if !version_sent {
                version_sent = true;
                sender.send(box [1u8].into_owned());
            }

            if greeting_bytes_read > SIGNATURE_SIZE && !type_sent {
                type_sent = true;
                match greeting_recv[10] {
                    ZMTP_1_0 | ZMTP_2_0 => {
                        sender.send(box [self.options.read().type_ as u8].into_owned());
                    }
                    _ => {
                        //TODO: error or ZMTP 3.0
                        sender.send(box [self.options.read().type_ as u8].into_owned());
                    }
                }
            }
        }

        if greeting_recv[0] != 0xff || greeting_recv[9] & 0x01 == 0 {
            return Err(ZmqError::new(consts::EPROTONOSUPPORT, "ZMTP 1.0 is not supported"));
        } else if greeting_recv[REVISION_POS] == ZMTP_1_0 {
            return Err(ZmqError::new(consts::EPROTONOSUPPORT, "ZMTP 1.0 is not supported"));
        } else {
            return Ok((V2Decoder::new(self.options.read().maxmsgsize), V2Encoder::new()));
        }
    }

    pub fn spawn_new(stream: TcpStream, options: Arc<RWLock<Options>>,
                     chan: Sender<ZmqResult<SocketMessage>>,
                     death_notifier: Option<Sender<u8>>) {
        spawn(proc() {
            let mut engine = StreamEngine {
                chan_to_socket: chan,
                stream: stream,
                options: options,
                _death_notifier: death_notifier,
            };
            let _ = engine.run();
        });
    }
}
