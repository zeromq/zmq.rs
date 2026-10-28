use super::*;
use std::sync::{Arc, Mutex};
use std::task::Waker;

type FirstPoll = Box<dyn FnOnce(&Waker) -> Poll<Option<usize>> + Send>;

struct ChangingStream {
    first_poll: Option<FirstPoll>,
    messages: VecDeque<usize>,
}

impl ChangingStream {
    fn ready(messages: &[usize]) -> Self {
        Self {
            first_poll: None,
            messages: messages.iter().copied().collect(),
        }
    }
}

impl Stream for ChangingStream {
    type Item = usize;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<usize>> {
        if let Some(first_poll) = self.first_poll.take() {
            return first_poll(cx.waker());
        }
        Poll::Ready(self.messages.pop_front())
    }
}

#[test]
fn remove_during_ready_poll_does_not_restore_the_stream() {
    let queue = FairQueue::<ChangingStream, usize>::new(false);
    let inner = Arc::downgrade(&queue.inner());
    let mut stream = ChangingStream::ready(&[11]);
    stream.first_poll = Some(Box::new(move |_| {
        inner.upgrade().unwrap().lock().remove(&1);
        Poll::Ready(Some(10))
    }));
    queue.inner().lock().insert(1, stream);

    let messages = futures::executor::block_on(queue.collect::<Vec<_>>());

    assert_eq!(messages, vec![(1, 10)]);
}

#[test]
fn clear_during_ready_poll_does_not_restore_the_stream() {
    let queue = FairQueue::<ChangingStream, usize>::new(false);
    let inner = Arc::downgrade(&queue.inner());
    let mut stream = ChangingStream::ready(&[11]);
    stream.first_poll = Some(Box::new(move |_| {
        inner.upgrade().unwrap().lock().clear();
        Poll::Ready(Some(10))
    }));
    queue.inner().lock().insert(1, stream);

    let messages = futures::executor::block_on(queue.collect::<Vec<_>>());

    assert_eq!(messages, vec![(1, 10)]);
}

#[test]
fn replacement_during_ready_poll_preserves_the_new_stream() {
    let queue = FairQueue::<ChangingStream, usize>::new(false);
    let inner = Arc::downgrade(&queue.inner());
    let mut stream = ChangingStream::ready(&[11]);
    stream.first_poll = Some(Box::new(move |_| {
        inner
            .upgrade()
            .unwrap()
            .lock()
            .insert(1, ChangingStream::ready(&[20]));
        Poll::Ready(Some(10))
    }));
    queue.inner().lock().insert(1, stream);

    let messages = futures::executor::block_on(queue.collect::<Vec<_>>());

    assert_eq!(messages, vec![(1, 10), (1, 20)]);
}

#[test]
fn remove_during_pending_poll_ignores_a_stale_self_wake() {
    let mut queue = FairQueue::<ChangingStream, usize>::new(false);
    let inner = Arc::downgrade(&queue.inner());
    let mut stream = ChangingStream::ready(&[11]);
    stream.first_poll = Some(Box::new(move |waker| {
        inner.upgrade().unwrap().lock().remove(&1);
        waker.wake_by_ref();
        Poll::Pending
    }));
    queue.inner().lock().insert(1, stream);
    let parent = noop_waker();

    let result = Pin::new(&mut queue).poll_next(&mut Context::from_waker(&parent));

    assert_eq!(result, Poll::Ready(None));
}

#[test]
fn replacement_during_pending_poll_preserves_the_new_stream() {
    let queue = FairQueue::<ChangingStream, usize>::new(false);
    let inner = Arc::downgrade(&queue.inner());
    let mut stream = ChangingStream::ready(&[11]);
    stream.first_poll = Some(Box::new(move |waker| {
        inner
            .upgrade()
            .unwrap()
            .lock()
            .insert(1, ChangingStream::ready(&[20]));
        waker.wake_by_ref();
        Poll::Pending
    }));
    queue.inner().lock().insert(1, stream);

    let messages = futures::executor::block_on(queue.collect::<Vec<_>>());

    assert_eq!(messages, vec![(1, 20)]);
}

#[test]
fn clear_during_eof_does_not_notify_a_removed_connection() {
    let mut queue = FairQueue::<ChangingStream, usize>::new(false);
    let disconnected = Arc::new(Mutex::new(Vec::new()));
    let events = disconnected.clone();
    queue.set_on_disconnect(move |key| events.lock().unwrap().push(key));
    let inner = Arc::downgrade(&queue.inner());
    let mut stream = ChangingStream::ready(&[]);
    stream.first_poll = Some(Box::new(move |_| {
        inner.upgrade().unwrap().lock().clear();
        Poll::Ready(None)
    }));
    queue.inner().lock().insert(1, stream);

    let messages = futures::executor::block_on(queue.collect::<Vec<_>>());

    assert!(messages.is_empty());
    assert!(disconnected.lock().unwrap().is_empty());
}

#[test]
fn replacement_during_eof_does_not_disconnect_the_new_stream() {
    let mut queue = FairQueue::<ChangingStream, usize>::new(false);
    let disconnected = Arc::new(Mutex::new(Vec::new()));
    let events = disconnected.clone();
    let callback_inner = Arc::downgrade(&queue.inner());
    queue.set_on_disconnect(move |key| {
        events.lock().unwrap().push(key);
        callback_inner.upgrade().unwrap().lock().remove(&key);
    });
    let inner = Arc::downgrade(&queue.inner());
    let mut stream = ChangingStream::ready(&[]);
    stream.first_poll = Some(Box::new(move |_| {
        inner
            .upgrade()
            .unwrap()
            .lock()
            .insert(1, ChangingStream::ready(&[20]));
        Poll::Ready(None)
    }));
    queue.inner().lock().insert(1, stream);

    let messages = futures::executor::block_on(queue.collect::<Vec<_>>());

    assert_eq!(messages, vec![(1, 20)]);
    assert_eq!(*disconnected.lock().unwrap(), vec![1]);
}

#[test]
fn current_stream_eof_notifies_once_outside_the_queue_lock() {
    let mut queue = FairQueue::<ChangingStream, usize>::new(false);
    let disconnected = Arc::new(Mutex::new(Vec::new()));
    let events = disconnected.clone();
    let inner = Arc::downgrade(&queue.inner());
    queue.set_on_disconnect(move |key| {
        inner
            .upgrade()
            .unwrap()
            .try_lock()
            .expect("disconnect callback must run outside the queue lock")
            .remove(&key);
        events.lock().unwrap().push(key);
    });
    queue.inner().lock().insert(1, ChangingStream::ready(&[10]));

    let messages = futures::executor::block_on(queue.collect::<Vec<_>>());

    assert_eq!(messages, vec![(1, 10)]);
    assert_eq!(*disconnected.lock().unwrap(), vec![1]);
}
