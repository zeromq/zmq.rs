use consts;
use msg::Msg;
use result::ZmqResult;


pub trait ZmqSocket {
    fn getsockopt(&self, option: consts::SocketOption) -> int;

    fn bind(&self, addr: &str) -> ZmqResult<()>;

    fn connect(&self, addr: &str) -> ZmqResult<()>;

    fn msg_recv(&mut self) -> ZmqResult<Box<Msg>>;

    fn msg_send(&mut self, msg: Box<Msg>) -> ZmqResult<()>;
}
