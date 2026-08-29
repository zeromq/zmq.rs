use zeromq::__async_rt as async_rt;
use zeromq::*;

use std::convert::TryInto;
use std::error::Error;
use std::time::Duration;

async fn wait_for_peer_handshake() {
    async_rt::task::sleep(Duration::from_millis(50)).await;
}

#[async_rt::test]
async fn pair_over_tcp_roundtrip() -> Result<(), Box<dyn Error>> {
    let mut server = PairSocket::new();
    let bound = server.bind("tcp://127.0.0.1:0").await?;

    let mut client = PairSocket::new();
    client.connect(&bound.to_string()).await?;
    wait_for_peer_handshake().await;

    client.send("ping".into()).await?;
    let msg = server.recv().await?;
    let payload: String = msg.try_into()?;
    assert_eq!(payload, "ping");

    server.send("pong".into()).await?;
    let msg = client.recv().await?;
    let payload: String = msg.try_into()?;
    assert_eq!(payload, "pong");

    Ok(())
}

#[async_rt::test]
async fn pair_over_inproc_roundtrip() -> Result<(), Box<dyn Error>> {
    let ctx = Context::new();

    let mut server_opts = SocketOptions::default();
    server_opts.context(ctx.clone());
    let mut server = PairSocket::with_options(server_opts);
    server.bind("inproc://pair-test").await?;

    let mut client_opts = SocketOptions::default();
    client_opts.context(ctx);
    let mut client = PairSocket::with_options(client_opts);
    client.connect("inproc://pair-test").await?;
    wait_for_peer_handshake().await;

    client.send("hello".into()).await?;
    let msg = server.recv().await?;
    let payload: String = msg.try_into()?;
    assert_eq!(payload, "hello");

    Ok(())
}

#[async_rt::test]
async fn pair_rejects_second_peer() -> Result<(), Box<dyn Error>> {
    let mut server = PairSocket::new();
    let bound = server.bind("tcp://127.0.0.1:0").await?;

    let mut first = PairSocket::new();
    first.connect(&bound.to_string()).await?;
    wait_for_peer_handshake().await;

    // Second connection attempt is accepted at TCP level but rejected by PAIR.
    let mut second = PairSocket::new();
    second.connect(&bound.to_string()).await?;
    wait_for_peer_handshake().await;

    first.send("from-first".into()).await?;
    let msg = server.recv().await?;
    let payload: String = msg.try_into()?;
    assert_eq!(payload, "from-first");

    // The second peer must not displace the first: server still talks to first.
    server.send("to-first".into()).await?;
    let msg = first.recv().await?;
    let payload: String = msg.try_into()?;
    assert_eq!(payload, "to-first");

    Ok(())
}
