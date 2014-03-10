use consts;


pub trait SocketBase {
    fn create() -> Self;

    fn getsockopt(&self, option_: consts::SocketOption) -> int;

    fn get_type(&self) -> consts::SocketType;
}

