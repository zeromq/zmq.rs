use socket_base::{InnerSocket, SocketMessage};
use result::ZmqResult;


pub trait Endpoint {
    fn get_chan<'a>(&'a self) -> &'a Receiver<ZmqResult<SocketMessage>>;

    fn handle(&mut self, msg: ZmqResult<SocketMessage>, socket: &mut InnerSocket);
}
