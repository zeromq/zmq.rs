use rand::Rng;
use tokio::net::TcpListener;
use tokio::prelude::*;

use std::time::Duration;
use zeromq::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rng = rand::thread_rng();
    println!("Start server");
    let mut socket = zeromq::PubSocket::bind("127.0.0.1:5556").await?;

    println!("Start sending loop");
    loop {
        let zipcode = rng.gen_range(10000, 10010);
        let temperature = rng.gen_range(-80, 135);
        let relhumidity = rng.gen_range(10, 60);
        let message = format!("{} {} {}", zipcode, temperature, relhumidity);
        socket.send(message.into_bytes()).await?;
        tokio::time::delay_for(Duration::from_millis(100)).await;
    }
    drop(socket);

    tokio::time::delay_for(Duration::from_secs(2)).await;

    println!("Shutdown");
    Ok(())
}
