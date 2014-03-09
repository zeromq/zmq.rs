use consts;

pub trait SocketBase {
    fn create(type_: consts::SocketType) -> ~SocketBase;
}

