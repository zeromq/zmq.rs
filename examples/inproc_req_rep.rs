//! Minimal REQ/REP over `inproc://` in one process.
//!
//! Both sockets share one [`zeromq::Context`]. Inproc does not work across
//! separate processes the way `tcp://` does.
//!
//! Run:
//! ```text
//! cargo run --example inproc_req_rep
//! ```

mod async_helpers;

use std::convert::TryInto;
use std::time::Duration;

use zeromq::*;

const ENDPOINT: &str = "inproc://req-rep-demo";

async fn run_rep(ctx: Context) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut opts = SocketOptions::default();
    opts.context(ctx);
    let mut rep = RepSocket::with_options(opts);
    rep.bind(ENDPOINT).await?;

    let request: String = rep.recv().await?.try_into()?;
    println!("REP received: {request:?}");

    rep.send(format!("{request} Reply").into()).await?;
    println!("REP sent reply");
    Ok(())
}

#[async_helpers::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = Context::new();
    let rep_ctx = ctx.clone();

    async_helpers::spawn(async move {
        if let Err(err) = run_rep(rep_ctx).await {
            eprintln!("REP task failed: {err}");
        }
    });

    // Let the REP bind before connect (connect also retries if needed).
    async_helpers::sleep(Duration::from_millis(10)).await;

    let mut opts = SocketOptions::default();
    opts.context(ctx);
    let mut req = ReqSocket::with_options(opts);
    req.connect(ENDPOINT).await?;
    println!("REQ connected");

    req.send("Hello".into()).await?;
    let reply: String = req.recv().await?.try_into()?;
    println!("REQ received: {reply:?}");

    Ok(())
}
