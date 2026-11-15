use super::*;

#[async_rt::test]
async fn full_subscriber_drops_new_messages_without_blocking_fast_subscriber() {
    let backend = test_backend();
    let slow_peer = PeerIdentity::new();
    let (slow_sender, mut slow_receiver) = mpsc::channel(1);
    let (fast_sender, mut fast_receiver) = mpsc::channel(1);
    insert_test_subscriber(&backend, slow_peer.clone(), vec![vec![]], slow_sender).await;
    insert_test_subscriber(&backend, PeerIdentity::new(), vec![vec![]], fast_sender).await;

    let mut fast_messages = Vec::new();
    for sequence in 0..100 {
        backend
            .fanout_message(ZmqMessage::from(sequence.to_string()))
            .await;
        if let Ok(Message::Message(message)) = fast_receiver.try_recv() {
            fast_messages.push(String::try_from(message).unwrap());
        }
    }
    let mut slow_messages = Vec::new();
    while let Ok(Message::Message(message)) = slow_receiver.try_recv() {
        slow_messages.push(String::try_from(message).unwrap());
    }

    assert_eq!(
        fast_messages,
        (0..100).map(|n| n.to_string()).collect::<Vec<_>>()
    );
    // futures mpsc allows the configured buffer plus one reserved slot per sender.
    assert_eq!(slow_messages, ["0", "1"]);
    assert!(backend.subscribers.get_sync(&slow_peer).is_some());
    assert_eq!(backend.subscriber_count.load(Ordering::Relaxed), 2);
}

#[async_rt::test]
async fn subscriber_resumes_in_fifo_order_after_full_queue_is_drained() {
    let backend = test_backend();
    let (sender, mut receiver) = mpsc::channel(1);
    insert_test_subscriber(&backend, PeerIdentity::new(), vec![vec![]], sender).await;
    for payload in ["first", "second", "dropped"] {
        backend.fanout_message(ZmqMessage::from(payload)).await;
    }
    let mut messages = Vec::new();
    while let Ok(Message::Message(message)) = receiver.try_recv() {
        messages.push(String::try_from(message).unwrap());
    }

    backend.fanout_message(ZmqMessage::from("resumed")).await;
    while let Ok(Message::Message(message)) = receiver.try_recv() {
        messages.push(String::try_from(message).unwrap());
    }

    assert_eq!(messages, ["first", "second", "resumed"]);
}
