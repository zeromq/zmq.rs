extern crate mio;
extern crate mioco;
//use options::Options;
use result::{ZmqError, ZmqResult};
use socket_base::SocketMessage;
//use stream_engine::StreamEngine;
//
//use std::old_io::Acceptor;
//use std::old_io::net::tcp::TcpAcceptor;
//use std::sync::Arc;
//use std::sync::RwLock;
//use std::sync::mpsc::Sender;
//use std::thread::Thread;
//
//
//static ACCEPT_TIMEOUT: u64 = 1000;
//
//
pub struct TcpListener {
//    acceptor: TcpAcceptor,
    chan_to_socket: mioco::MailboxOuterEnd<ZmqResult<SocketMessage>>,
//    options: Arc<RwLock<Options>>,
}

impl TcpListener {
//    fn run(&mut self) -> Result<(), ZmqResult<SocketMessage>> {
//        loop {
//            self.acceptor.set_timeout(Some(ACCEPT_TIMEOUT));
//            match self.acceptor.accept() {
//                Ok(stream) => {
//                    if self.chan_to_socket.send_opt(Ok(SocketMessage::Ping)).is_err() {
//                        return Ok(());
//                    }
//
//                    let options = self.options.clone();
//                    StreamEngine::spawn_new(stream, options, self.chan_to_socket.clone(), None);
//                }
//                Err(e) =>
//                    try!(self.chan_to_socket.send_opt(Err(ZmqError::from_io_error(e)))),
//            }
//        }
//    }
//
    pub fn spawn_new(listener: mio::tcp::TcpListener,
                     chan: &mioco::MailboxOuterEnd<ZmqResult<SocketMessage>>) {
//                     options: Arc<RwLock<Options>>) {
//        mioco::spawn(move || {
//            let mut listener = TcpListener {
//                acceptor: acceptor,
//                options: options,
//                chan_to_socket: chan,
//            };
//            let _ = listener.run();
//        });
    }
}
