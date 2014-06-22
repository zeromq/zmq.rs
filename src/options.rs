pub struct Options {
    identity_size: u8,
    //identity: [u8,..256],
}

impl Options {
    pub fn new() -> Options {
        Options {
            identity_size: 0,
        }
    }
}
