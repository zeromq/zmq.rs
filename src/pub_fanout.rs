use crate::codec::{Message, ZmqCodec, ZmqFramedWrite};
use crate::error::ZmqResult;
use crate::message::ZmqMessage;
use crate::util::PeerIdentity;

use asynchronous_codec::Encoder;
use bytes::BytesMut;
use futures::{channel::mpsc, io::AsyncWriteExt};

use std::collections::HashMap;
use std::io::ErrorKind;

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
    send_queue: ZmqFramedWrite,
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
            } => self.peer_connected(peer_id, send_queue),
            FanoutEvent::Subscription { peer_id, change } => {
                self.apply_subscription(&peer_id, change);
            }
            FanoutEvent::PeerDisconnected(peer_id) => {
                self.peer_disconnected(&peer_id);
            }
        }
    }

    pub(crate) fn peer_connected(&mut self, peer_id: PeerIdentity, send_queue: ZmqFramedWrite) {
        self.peers.insert(
            peer_id,
            FanoutPeer {
                subscriptions: Vec::new(),
                all_subscription_count: 0,
                send_queue,
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

        for (peer_id, peer) in self.peers.iter_mut() {
            if !peer.is_subscribed_to(first_frame) {
                continue;
            }

            let peer_id = peer_id.clone();
            let res = peer.send_queue.write_all(encoded.as_ref()).await;
            handle_write_result(peer_id, res, &mut dead_peers);
        }

        for peer_id in &dead_peers {
            self.peers.remove(peer_id);
        }

        Ok(dead_peers)
    }
}

fn encode_message(message: &ZmqMessage) -> ZmqResult<BytesMut> {
    let mut encoded = BytesMut::new();
    ZmqCodec::new().encode(Message::Message(message.clone()), &mut encoded)?;
    Ok(encoded)
}

fn handle_write_result(
    peer_id: PeerIdentity,
    result: std::io::Result<()>,
    dead_peers: &mut Vec<PeerIdentity>,
) {
    match result {
        Ok(()) => {}
        Err(e) => {
            if matches!(e.kind(), ErrorKind::BrokenPipe | ErrorKind::ConnectionReset) {
                dead_peers.push(peer_id);
            } else {
                log::error!("Error sending message: {:?}", e);
            }
        }
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
    use crate::codec::FrameableWrite;

    #[test]
    fn duplicate_subscription_requires_matching_unsubscribe_events() {
        let mut state = FanoutState::default();
        let peer_id = PeerIdentity::new();
        state.peers.insert(
            peer_id.clone(),
            FanoutPeer {
                subscriptions: Vec::new(),
                all_subscription_count: 0,
                send_queue: sink_write(),
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
                send_queue: sink_write(),
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

    fn sink_write() -> ZmqFramedWrite {
        let writer: Box<dyn FrameableWrite> = Box::new(futures::io::sink());
        ZmqFramedWrite::new(writer)
    }
}
