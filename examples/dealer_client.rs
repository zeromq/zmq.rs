mod async_helpers;

use futures::StreamExt;
use std::{error::Error, time::Duration};
use zeromq::prelude::*;

#[async_helpers::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut client = zeromq::DealerSocket::new();
    let mut monitor = client.monitor();
    async_helpers::spawn(async move {
        while let Some(event) = monitor.next().await {
            dbg!(event);
        }
    });

    client.connect("tcp://127.0.0.1:5559").await?;

    loop {
        let result = client.send("Test message".into()).await;
        dbg!(result);
        async_helpers::sleep(Duration::from_secs(1)).await;
    }
}
