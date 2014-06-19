use result::ZmqResult;
use socket_base::{SocketBase, SocketMessage};


pub trait Endpoint {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>>;

    fn in_event(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut SocketBase);

    fn is_critical(&self) -> bool;
}
