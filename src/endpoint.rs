use result::ZmqResult;
use socket_base::{SocketBase, SocketMessage};


pub trait Endpoint {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>>;

    fn in_event(&mut self, _msg: ZmqResult<SocketMessage>, _socket: &mut SocketBase) { }

    fn is_critical(&self) -> bool { false }
}
