use zeromq::__async_rt as async_rt;
use zeromq::*;

use std::convert::TryInto;
use std::error::Error;
use std::time::Duration;

#[async_rt::test]
async fn push_pull_inproc_roundtrip() -> Result<(), Box<dyn Error>> {
    let ctx = Context::new();

    let mut pull_opts = SocketOptions::default();
    pull_opts.context(ctx.clone());
    let mut pull = PullSocket::with_options(pull_opts);
    pull.bind("inproc://push-pull").await?;

    let mut push_opts = SocketOptions::default();
    push_opts.context(ctx);
    let mut push = PushSocket::with_options(push_opts);
    push.connect("inproc://push-pull").await?;

    // Give the accept task a moment to register the peer.
    async_rt::task::sleep(Duration::from_millis(50)).await;

    push.send("hello-inproc".into()).await?;
    let msg = pull.recv().await?;
    let payload: String = msg.try_into()?;
    assert_eq!(payload, "hello-inproc");

    Ok(())
}

#[async_rt::test]
async fn inproc_requires_context() -> Result<(), Box<dyn Error>> {
    let mut pull = PullSocket::new();
    let err = pull.bind("inproc://no-context").await.unwrap_err();
    assert!(matches!(err, ZmqError::Socket(_)));
    Ok(())
}
