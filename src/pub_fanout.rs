use crate::codec::{Message, ZmqFramedWrite};
use crate::message::ZmqMessage;
use crate::util::PeerIdentity;
use crate::{async_rt, write_queue::write_message_queue};

use futures::channel::mpsc;

/// Bounded per-peer queue depth. Mirrors the PUB fanout queue so XPUB shares
/// the same drop-on-slow-subscriber behavior.
const FANOUT_SEND_QUEUE_CAPACITY: usize = 100_000;

pub(crate) type FanoutEventSender = mpsc::UnboundedSender<FanoutEvent>;
pub(crate) type FanoutEventReceiver = mpsc::UnboundedReceiver<FanoutEvent>;

pub(crate) enum SubscriptionChange {
    Subscribe(Vec<u8>),
    Unsubscribe(Vec<u8>),
}

/// Backend-to-socket fanout events.
///
/// The backend owns the I/O lifecycle (accept, disconnect) but does not touch
/// the send path. It hands the socket a bounded write queue on connect and a
/// peer-id on disconnect. Subscription changes for XPUB are applied by the
/// socket itself from the recv path, so they are not carried as events here.
pub(crate) enum FanoutEvent {
    PeerConnected {
        peer_id: PeerIdentity,
        send_queue: ZmqFramedWrite,
    },
    PeerDisconnected(PeerIdentity),
}

struct FanoutPeer {
    subscriptions: Vec<Vec<u8>>,
    send_queue: mpsc::Sender<Message>,
}

impl FanoutPeer {
    fn is_subscribed_to(&self, first_frame: &[u8]) -> bool {
        self.subscriptions.iter().any(|sub_filter| {
            sub_filter.len() <= first_frame.len()
                && sub_filter.as_slice() == &first_frame[..sub_filter.len()]
        })
    }
}

/// Socket-owned fanout connection and subscription state.
///
/// Steady-state sends iterate this map directly. No backend scc lookup, no
/// async writer mutex. Each peer drains its bounded queue on a dedicated writer
/// task via [`write_message_queue`], so a full or closed queue drops one
/// message rather than blocking the publisher.
pub(crate) struct FanoutState {
    peers: std::collections::HashMap<PeerIdentity, FanoutPeer>,
    /// Cloned into each peer's writer task so a transport write failure evicts
    /// the dead peer from the fanout map (the writer emits `PeerDisconnected`).
    /// This matches the cleanup the PUB writer does on its backend and keeps a
    /// half-open peer (write dead, read still open) from leaking its slot.
    event_sender: FanoutEventSender,
}

impl FanoutState {
    pub(crate) fn new(event_sender: FanoutEventSender) -> Self {
        Self {
            peers: std::collections::HashMap::new(),
            event_sender,
        }
    }

    pub(crate) fn drain_events(&mut self, events: &mut FanoutEventReceiver) {
        while let Ok(event) = events.try_recv() {
            self.apply_event(event);
        }
    }

    fn apply_event(&mut self, event: FanoutEvent) {
        match event {
            FanoutEvent::PeerConnected {
                peer_id,
                send_queue,
            } => self.peer_connected(peer_id, send_queue),
            FanoutEvent::PeerDisconnected(peer_id) => self.peer_disconnected(&peer_id),
        }
    }

    fn peer_connected(&mut self, peer_id: PeerIdentity, send_queue: ZmqFramedWrite) {
        let (queue_sender, queue_receiver) = mpsc::channel(FANOUT_SEND_QUEUE_CAPACITY);
        let writer_peer_id = peer_id.clone();
        let writer_events = self.event_sender.clone();
        async_rt::task::spawn(async move {
            if let Err(error) = write_message_queue(queue_receiver, send_queue).await {
                log::debug!(
                    "Error sending message to fanout peer {:?}: {:?}",
                    writer_peer_id,
                    error
                );
                // Transport write failed, so evict the peer instead of leaving a
                // dead slot that silently drops every future send. Drained on the
                // next send/recv, the same path backend disconnects flow through.
                let _ = writer_events.unbounded_send(FanoutEvent::PeerDisconnected(writer_peer_id));
            }
        });
        self.peers.insert(
            peer_id,
            FanoutPeer {
                subscriptions: Vec::new(),
                send_queue: queue_sender,
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
                peer.subscriptions.push(subscription);
            }
            SubscriptionChange::Unsubscribe(subscription) => {
                if let Some(index) = peer
                    .subscriptions
                    .iter()
                    .position(|existing| existing == &subscription)
                {
                    peer.subscriptions.remove(index);
                }
            }
        }
    }

    /// Queue `message` for every peer whose subscriptions match `first_frame`.
    ///
    /// A full or closed peer queue drops the message for that peer; the writer
    /// task owns transport error detection and disconnect cleanup.
    pub(crate) fn send_matching(&mut self, first_frame: &[u8], message: &ZmqMessage) {
        for peer in self.peers.values_mut() {
            if !peer.is_subscribed_to(first_frame) {
                continue;
            }

            match peer.send_queue.try_send(Message::Message(message.clone())) {
                Ok(()) => {}
                Err(error) => {
                    // Slow or gone subscriber: drop the message rather than backpressure the publisher.
                    drop(error.into_inner());
                }
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

    fn test_state() -> FanoutState {
        // The writer-eviction sender is unused by these tests (peers are inserted
        // directly, no writer task is spawned), so a detached sender is fine.
        let (event_sender, _events) = mpsc::unbounded();
        FanoutState::new(event_sender)
    }

    fn insert_peer(state: &mut FanoutState, peer_id: PeerIdentity) {
        let (queue_sender, _queue_receiver) = mpsc::channel(8);
        state.peers.insert(
            peer_id,
            FanoutPeer {
                subscriptions: Vec::new(),
                send_queue: queue_sender,
            },
        );
    }

    #[test]
    fn duplicate_subscription_requires_matching_unsubscribe_events() {
        let mut state = test_state();
        let peer_id = PeerIdentity::new();
        insert_peer(&mut state, peer_id.clone());

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
    fn send_matching_skips_unsubscribed_peers() {
        let mut state = test_state();
        let peer_id = PeerIdentity::new();
        let (queue_sender, mut queue_receiver) = mpsc::channel(8);
        state.peers.insert(
            peer_id.clone(),
            FanoutPeer {
                subscriptions: vec![b"topic".to_vec()],
                send_queue: queue_sender,
            },
        );

        state.send_matching(b"other", &ZmqMessage::from("other-payload"));
        assert!(queue_receiver.try_recv().is_err());

        state.send_matching(b"topic.a", &ZmqMessage::from("topic.a payload"));
        let Message::Message(message) = queue_receiver.try_recv().expect("queued message") else {
            panic!("expected a queued message");
        };
        assert_eq!(
            message.get(0).unwrap().as_ref(),
            b"topic.a payload".as_slice()
        );
    }

    #[test]
    fn send_matching_drops_for_dead_peer_and_still_serves_healthy_peer() {
        let mut state = test_state();

        // Dead peer: its writer task is gone, so the receiver is dropped and
        // try_send fails. The message is dropped without disturbing other peers.
        let dead = PeerIdentity::new();
        let (dead_sender, dead_receiver) = mpsc::channel(8);
        drop(dead_receiver);
        state.peers.insert(
            dead,
            FanoutPeer {
                subscriptions: vec![b"topic".to_vec()],
                send_queue: dead_sender,
            },
        );

        let healthy = PeerIdentity::new();
        let (healthy_sender, mut healthy_receiver) = mpsc::channel(8);
        state.peers.insert(
            healthy,
            FanoutPeer {
                subscriptions: vec![b"topic".to_vec()],
                send_queue: healthy_sender,
            },
        );

        state.send_matching(b"topic.a", &ZmqMessage::from("payload"));

        let Message::Message(message) = healthy_receiver.try_recv().expect("queued message") else {
            panic!("expected a queued message for the healthy peer");
        };
        assert_eq!(message.get(0).unwrap().as_ref(), b"payload".as_slice());
    }
}
