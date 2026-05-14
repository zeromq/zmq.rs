use crate::codec::{CodecError, Message, ZmqFramedWrite};
use crate::util::PeerIdentity;
use crate::ZmqMessage;

use futures::future::join_all;
use futures::lock::Mutex as AsyncMutex;
use futures::SinkExt;

use std::pin::Pin;
use std::sync::Arc;

pub(crate) type SharedSendQueue = Arc<AsyncMutex<Pin<Box<ZmqFramedWrite>>>>;

pub(crate) async fn send_message_to_targets(
    targets: Vec<(PeerIdentity, SharedSendQueue)>,
    message: ZmqMessage,
) -> Vec<(PeerIdentity, Result<(), CodecError>)> {
    join_all(targets.into_iter().map(|(peer_id, send_queue)| {
        let message = message.clone();
        async move {
            let result = send_queue
                .lock()
                .await
                .as_mut()
                .send(Message::Message(message))
                .await;
            (peer_id, result)
        }
    }))
    .await
}
