use zeromq::__async_rt as async_rt;
use zeromq::prelude::*;
use zeromq::{RepSocket, ReqSocket, SocketOptions, ZmqError, ZmqMessage};

const LIMIT: usize = 1024;

/// A REQ client limited to `LIMIT` bytes asks a REP server that answers with
/// `reply_len` bytes, and returns what its receive gives.
async fn request_reply(reply_len: usize) -> Result<ZmqMessage, ZmqError> {
    let mut rep = RepSocket::new();
    let endpoint = rep.bind("tcp://localhost:0").await.unwrap();
    async_rt::task::spawn(async move {
        rep.recv().await.unwrap();
        rep.send(ZmqMessage::from(vec![7u8; reply_len]))
            .await
            .unwrap();
    });
    let mut options = SocketOptions::default();
    options.max_message_size(LIMIT);
    let mut req = ReqSocket::with_options(options);
    req.connect(&endpoint.to_string()).await.unwrap();
    req.send("ping".into()).await.unwrap();
    req.recv().await
}

#[async_rt::test]
async fn a_reply_within_the_limit_is_received() {
    let reply = request_reply(LIMIT).await.unwrap();
    assert_eq!(reply.get(0).unwrap().len(), LIMIT);
}

#[async_rt::test]
async fn a_reply_over_the_limit_fails_the_receive() {
    let err = request_reply(LIMIT + 1).await.unwrap_err();
    assert!(matches!(err, ZmqError::Codec(_)), "{err:?}");
}
