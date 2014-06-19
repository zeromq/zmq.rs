use result::ZmqResult;
use socket_base::{SocketBase, SocketMessage};


pub trait Endpoint {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>>;

    fn handle(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut SocketBase);
}
