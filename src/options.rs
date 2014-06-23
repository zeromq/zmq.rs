use consts;
use consts::SocketOption;


pub struct Options {
    //  Socket identity.
    pub identity_size: u8,
    //identity: [u8,..256],

    //  Socket type.
    pub type_: int,
}

impl Options {
    pub fn new() -> Options {
        Options {
            identity_size: 0,
            type_: -1,
        }
    }

    pub fn getsockopt(&self, option: SocketOption) -> int {
        match option {
            consts::TYPE => self.type_ as int,
        }
    }
}
