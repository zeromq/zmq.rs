use consts;
use consts::SocketOption;


pub struct Options {
    //  Socket identity.
    pub identity_size: u8,
    //identity: [u8,..256],

    //  Socket type.
    pub type_: int,

    //  Minimum interval between attempts to reconnect, in milliseconds.
    //  Default 100ms
    pub reconnect_ivl: u64,

    //  Maximum interval between attempts to reconnect, in milliseconds.
    //  Default 0 (unused)
    pub reconnect_ivl_max: u64,

    //  Maximal size of message to handle.
    pub maxmsgsize: i64,
}

impl Options {
    pub fn new() -> Options {
        Options {
            identity_size: 0,
            type_: -1,
            maxmsgsize: -1,
            reconnect_ivl: 100,
            reconnect_ivl_max: 0,
        }
    }

    pub fn getsockopt(&self, option: SocketOption) -> int {
        match option {
            consts::TYPE => self.type_ as int,
        }
    }
}
