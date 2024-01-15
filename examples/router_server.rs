mod async_helpers;

use std::{convert::TryFrom, error::Error};
use zeromq::{prelude::*, util::PeerIdentity, SocketOptions};

#[async_helpers::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut options = SocketOptions::default();
    options.peer_identity(PeerIdentity::try_from(Vec::from("SomeCustomId")).unwrap());
    let mut frontend = zeromq::RouterSocket::with_options(options);
    frontend.bind("tcp://127.0.0.1:5559").await?;

    loop {
        let message = frontend.recv().await?;
        dbg!(message);
    }
}
