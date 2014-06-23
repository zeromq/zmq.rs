pub static MORE: u8 = 1;
pub static COMMAND: u8 = 2;


#[deriving(Show)]
pub struct Msg {
    pub data: Vec<u8>,
    pub flags: u8,
}

impl Msg {
    pub fn new(size: uint) -> Msg {
        Msg {
            data: Vec::with_capacity(size),
            flags: 0,
        }
    }
}
