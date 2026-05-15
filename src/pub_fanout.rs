use crate::async_rt;
use crate::codec::{Message, ZmqCodec, ZmqFramedWrite};
use crate::error::ZmqResult;
use crate::message::ZmqMessage;
use crate::util::PeerIdentity;

use asynchronous_codec::Encoder;
use bytes::{Bytes, BytesMut};
use futures::channel::oneshot;
use futures::{channel::mpsc, io::AsyncWriteExt, SinkExt, StreamExt};

use std::collections::HashMap;
use std::io::ErrorKind;

const PEER_QUEUE_CAPACITY: usize = 1024;
const PEER_BATCH_MAX_MESSAGES: usize = 64;
const PEER_BATCH_MAX_BYTES: usize = 64 * 1024;
const PEER_SYNC_WRITE_THRESHOLD: usize = 1024;

pub(crate) type FanoutEventSender = mpsc::UnboundedSender<FanoutEvent>;
pub(crate) type FanoutEventReceiver = mpsc::UnboundedReceiver<FanoutEvent>;

pub(crate) enum SubscriptionChange {
    Subscribe(Vec<u8>),
    Unsubscribe(Vec<u8>),
}

pub(crate) enum FanoutEvent {
    PeerConnected {
        peer_id: PeerIdentity,
        send_queue: ZmqFramedWrite,
        fanout_events: FanoutEventSender,
    },
    Subscription {
        peer_id: PeerIdentity,
        change: SubscriptionChange,
    },
    PeerDisconnected(PeerIdentity),
}

struct FanoutPeer {
    subscriptions: Vec<Vec<u8>>,
    all_subscription_count: usize,
    writer: FanoutPeerWriter,
}

impl FanoutPeer {
    fn is_subscribed_to(&self, first_frame: &[u8]) -> bool {
        if self.all_subscription_count > 0 {
            return true;
        }

        self.subscriptions.iter().any(|sub_filter| {
            sub_filter.len() <= first_frame.len()
                && sub_filter.as_slice() == &first_frame[..sub_filter.len()]
        })
    }
}

struct FanoutPeerWriter {
    sender: mpsc::Sender<PeerWrite>,
    large_accepted: Option<oneshot::Receiver<()>>,
}

struct PeerWrite {
    encoded: Bytes,
    accepted: Option<oneshot::Sender<()>>,
}

impl FanoutPeerWriter {
    fn spawn(
        peer_id: PeerIdentity,
        send_queue: ZmqFramedWrite,
        fanout_events: FanoutEventSender,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(PEER_QUEUE_CAPACITY);
        async_rt::task::spawn(peer_writer_task(
            peer_id,
            send_queue,
            receiver,
            fanout_events,
        ));
        Self {
            sender,
            large_accepted: None,
        }
    }

    async fn send(&mut self, encoded: Bytes, limit_large_inflight: bool) -> Result<(), ()> {
        let accepted = if limit_large_inflight {
            if let Some(previous) = self.large_accepted.take() {
                previous.await.map_err(|_canceled| ())?;
            }
            let (sender, receiver) = oneshot::channel();
            self.large_accepted = Some(receiver);
            Some(sender)
        } else {
            None
        };

        self.sender
            .send(PeerWrite { encoded, accepted })
            .await
            .map_err(|_send_error| ())
    }
}

async fn peer_writer_task(
    peer_id: PeerIdentity,
    mut send_queue: ZmqFramedWrite,
    mut receiver: mpsc::Receiver<PeerWrite>,
    fanout_events: FanoutEventSender,
) {
    while let Some(first) = receiver.next().await {
        if should_defer_peer_batch() {
            async_rt::task::yield_now().await;
        }

        match write_batch(&mut send_queue, &mut receiver, first).await {
            Ok(()) => {}
            Err(error) => {
                log_peer_write_error(&peer_id, &error);
                let _ = fanout_events.unbounded_send(FanoutEvent::PeerDisconnected(peer_id));
                break;
            }
        }
    }
}

async fn write_batch(
    send_queue: &mut ZmqFramedWrite,
    receiver: &mut mpsc::Receiver<PeerWrite>,
    first: PeerWrite,
) -> std::io::Result<()> {
    let first_encoded = first.encoded;
    let mut accepted = Vec::new();
    if let Some(accept) = first.accepted {
        accepted.push(accept);
    }

    let mut message_count = 1;
    let mut byte_count = first_encoded.len();
    let mut batch = None;

    while message_count < PEER_BATCH_MAX_MESSAGES && byte_count < PEER_BATCH_MAX_BYTES {
        let next = match receiver.try_recv() {
            Ok(next) => next,
            Err(_) => break,
        };
        let next_encoded = next.encoded;
        if let Some(accept) = next.accepted {
            accepted.push(accept);
        }

        let batch = batch.get_or_insert_with(|| {
            let mut batch = BytesMut::with_capacity(first_encoded.len() + next_encoded.len());
            batch.extend_from_slice(first_encoded.as_ref());
            batch
        });
        batch.extend_from_slice(next_encoded.as_ref());
        message_count += 1;
        byte_count += next_encoded.len();
    }

    for accept in accepted {
        let _ = accept.send(());
    }

    match batch {
        Some(batch) => send_queue.write_all(batch.as_ref()).await?,
        None => send_queue.write_all(first_encoded.as_ref()).await?,
    };

    Ok(())
}

#[derive(Default)]
pub(crate) struct FanoutState {
    peers: HashMap<PeerIdentity, FanoutPeer>,
}

impl FanoutState {
    pub(crate) fn drain_events(&mut self, events: &mut FanoutEventReceiver) {
        while let Ok(event) = events.try_recv() {
            self.apply_event(event);
        }
    }

    pub(crate) fn apply_event(&mut self, event: FanoutEvent) {
        match event {
            FanoutEvent::PeerConnected {
                peer_id,
                send_queue,
                fanout_events,
            } => self.peer_connected(peer_id, send_queue, fanout_events),
            FanoutEvent::Subscription { peer_id, change } => {
                self.apply_subscription(&peer_id, change);
            }
            FanoutEvent::PeerDisconnected(peer_id) => {
                self.peer_disconnected(&peer_id);
            }
        }
    }

    pub(crate) fn peer_connected(
        &mut self,
        peer_id: PeerIdentity,
        send_queue: ZmqFramedWrite,
        fanout_events: FanoutEventSender,
    ) {
        let writer = FanoutPeerWriter::spawn(peer_id.clone(), send_queue, fanout_events);
        self.peers.insert(
            peer_id,
            FanoutPeer {
                subscriptions: Vec::new(),
                all_subscription_count: 0,
                writer,
            },
        );
    }

    pub(crate) fn peer_disconnected(&mut self, peer_id: &PeerIdentity) {
        self.peers.remove(peer_id);
    }

    pub(crate) fn apply_subscription(
        &mut self,
        peer_id: &PeerIdentity,
        change: SubscriptionChange,
    ) {
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };

        match change {
            SubscriptionChange::Subscribe(subscription) => {
                if subscription.is_empty() {
                    peer.all_subscription_count += 1;
                }
                peer.subscriptions.push(subscription);
            }
            SubscriptionChange::Unsubscribe(subscription) => {
                if let Some(index) = peer
                    .subscriptions
                    .iter()
                    .position(|existing| existing == &subscription)
                {
                    let removed = peer.subscriptions.remove(index);
                    if removed.is_empty() {
                        peer.all_subscription_count = peer.all_subscription_count.saturating_sub(1);
                    }
                }
            }
        }
    }

    pub(crate) async fn send_matching(
        &mut self,
        first_frame: &[u8],
        message: &ZmqMessage,
    ) -> ZmqResult<Vec<PeerIdentity>> {
        let mut dead_peers = Vec::new();
        let encoded = encode_message(message)?;
        let limit_large_inflight = encoded.len() >= PEER_SYNC_WRITE_THRESHOLD;
        let mut queued = false;

        for (peer_id, peer) in self.peers.iter_mut() {
            if !peer.is_subscribed_to(first_frame) {
                continue;
            }

            let peer_id = peer_id.clone();
            match peer
                .writer
                .send(encoded.clone(), limit_large_inflight)
                .await
            {
                Ok(()) => {
                    queued = true;
                }
                Err(_) => {
                    dead_peers.push(peer_id);
                }
            }
        }

        for peer_id in &dead_peers {
            self.peers.remove(peer_id);
        }

        if queued && should_yield_after_enqueue() {
            async_rt::task::yield_now().await;
        }

        Ok(dead_peers)
    }
}

fn should_defer_peer_batch() -> bool {
    #[cfg(feature = "tokio-runtime")]
    {
        !matches!(
            tokio::runtime::Handle::current().runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread
        )
    }

    #[cfg(not(feature = "tokio-runtime"))]
    {
        false
    }
}

fn should_yield_after_enqueue() -> bool {
    #[cfg(feature = "tokio-runtime")]
    {
        matches!(
            tokio::runtime::Handle::current().runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread
        )
    }

    #[cfg(not(feature = "tokio-runtime"))]
    {
        true
    }
}

fn encode_message(message: &ZmqMessage) -> ZmqResult<Bytes> {
    let mut encoded = BytesMut::new();
    ZmqCodec::new().encode(Message::Message(message.clone()), &mut encoded)?;
    Ok(encoded.freeze())
}

fn log_peer_write_error(peer_id: &PeerIdentity, error: &std::io::Error) {
    if matches!(
        error.kind(),
        ErrorKind::BrokenPipe | ErrorKind::ConnectionReset
    ) {
        log::debug!("Peer {:?} disconnected during PUB fanout write", peer_id);
    } else {
        log::error!("Error sending message to peer {:?}: {:?}", peer_id, error);
    }
}

pub(crate) fn subscription_change(message: &ZmqMessage) -> Option<SubscriptionChange> {
    let frame = message.get(0)?;
    if message.len() != 1 || frame.is_empty() {
        return None;
    }

    match frame[0] {
        1 => Some(SubscriptionChange::Subscribe(frame[1..].to_vec())),
        0 => Some(SubscriptionChange::Unsubscribe(frame[1..].to_vec())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_subscription_requires_matching_unsubscribe_events() {
        let mut state = FanoutState::default();
        let peer_id = PeerIdentity::new();
        state.peers.insert(
            peer_id.clone(),
            FanoutPeer {
                subscriptions: Vec::new(),
                all_subscription_count: 0,
                writer: sink_writer(),
            },
        );

        state.apply_subscription(&peer_id, SubscriptionChange::Subscribe(b"dup".to_vec()));
        state.apply_subscription(&peer_id, SubscriptionChange::Subscribe(b"dup".to_vec()));

        let peer = state.peers.get(&peer_id).unwrap();
        assert!(peer.is_subscribed_to(b"dup-after-subscribe"));

        state.apply_subscription(&peer_id, SubscriptionChange::Unsubscribe(b"dup".to_vec()));
        let peer = state.peers.get(&peer_id).unwrap();
        assert!(peer.is_subscribed_to(b"dup-after-one-unsubscribe"));

        state.apply_subscription(&peer_id, SubscriptionChange::Unsubscribe(b"dup".to_vec()));
        let peer = state.peers.get(&peer_id).unwrap();
        assert!(!peer.is_subscribed_to(b"dup-after-two-unsubscribes"));
    }

    #[test]
    fn duplicate_empty_subscription_requires_matching_unsubscribe_events() {
        let mut state = FanoutState::default();
        let peer_id = PeerIdentity::new();
        state.peers.insert(
            peer_id.clone(),
            FanoutPeer {
                subscriptions: Vec::new(),
                all_subscription_count: 0,
                writer: sink_writer(),
            },
        );

        state.apply_subscription(&peer_id, SubscriptionChange::Subscribe(Vec::new()));
        state.apply_subscription(&peer_id, SubscriptionChange::Subscribe(Vec::new()));

        let peer = state.peers.get(&peer_id).unwrap();
        assert!(peer.is_subscribed_to(b"anything-after-empty-subscribe"));

        state.apply_subscription(&peer_id, SubscriptionChange::Unsubscribe(Vec::new()));
        let peer = state.peers.get(&peer_id).unwrap();
        assert!(peer.is_subscribed_to(b"anything-after-one-unsubscribe"));

        state.apply_subscription(&peer_id, SubscriptionChange::Unsubscribe(Vec::new()));
        let peer = state.peers.get(&peer_id).unwrap();
        assert!(!peer.is_subscribed_to(b"anything-after-two-unsubscribes"));
    }

    fn sink_writer() -> FanoutPeerWriter {
        let (sender, _receiver_guard) = mpsc::channel(1);
        FanoutPeerWriter {
            sender,
            large_accepted: None,
        }
    }
}
