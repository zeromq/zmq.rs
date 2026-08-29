//! Multithreaded relay (Guide Ch.2) using `PAIR` over `inproc://`.
//!
//! Port of zguide `mtrelay.c`: three steps signal readiness through exclusive
//! pairs that share one [`zeromq::Context`].
//!
//! Run with:
//! ```text
//! cargo run --example mtrelay
//! ```

mod async_helpers;

use std::convert::TryInto;
use std::error::Error;

use zeromq::*;

fn pair_with_context(ctx: &Context) -> PairSocket {
    let mut opts = SocketOptions::default();
    opts.context(ctx.clone());
    PairSocket::with_options(opts)
}

async fn step1(ctx: Context) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut xmitter = pair_with_context(&ctx);
    // Retries while step2 finishes binding (ConnectionRefused → backoff).
    xmitter.connect("inproc://step2").await?;
    println!("Step 1 ready, signaling step 2");
    xmitter.send("READY".into()).await?;
    Ok(())
}

async fn step2(ctx: Context) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut receiver = pair_with_context(&ctx);
    receiver.bind("inproc://step2").await?;

    let step1_ctx = ctx.clone();
    async_helpers::spawn(async move {
        if let Err(err) = step1(step1_ctx).await {
            eprintln!("step1 failed: {err}");
        }
    });

    let msg = receiver.recv().await?;
    let _: String = msg.try_into()?;
    drop(receiver);

    let mut xmitter = pair_with_context(&ctx);
    // Retries while main finishes binding `inproc://step3`.
    xmitter.connect("inproc://step3").await?;
    println!("Step 2 ready, signaling step 3");
    xmitter.send("READY".into()).await?;
    Ok(())
}

#[async_helpers::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let ctx = Context::new();

    let mut receiver = pair_with_context(&ctx);
    receiver.bind("inproc://step3").await?;

    let step2_ctx = ctx.clone();
    async_helpers::spawn(async move {
        if let Err(err) = step2(step2_ctx).await {
            eprintln!("step2 failed: {err}");
        }
    });

    let msg = receiver.recv().await?;
    let _: String = msg.try_into()?;

    println!("Test successful!");
    Ok(())
}
