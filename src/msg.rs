pub static MORE: u8 = 1;
pub static COMMAND: u8 = 2;


#[derive(Debug)]
pub struct Msg {
    pub data: Vec<u8>,
    pub flags: u8,
}

impl Msg {
    pub fn new(size: usize) -> Msg {
        Msg {
            data: Vec::with_capacity(size),
            flags: 0,
        }
    }
}
