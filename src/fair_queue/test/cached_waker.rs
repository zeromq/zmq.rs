use super::*;
use futures::task::{waker, ArcWake};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::Waker;

#[derive(Default)]
struct StreamState {
    wakers: Vec<Waker>,
    message: Option<usize>,
}

struct RecordingStream {
    state: Arc<Mutex<StreamState>>,
    self_wake: bool,
}

impl Stream for RecordingStream {
    type Item = usize;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<usize>> {
        let mut state = self.state.lock();
        state.wakers.push(cx.waker().clone());
        if self.self_wake {
            cx.waker().wake_by_ref();
        }
        state
            .message
            .take()
            .map_or(Poll::Pending, |message| Poll::Ready(Some(message)))
    }
}

#[test]
fn connection_polls_preserve_waker_identity() {
    let state = Arc::new(Mutex::new(StreamState::default()));
    let mut queue = FairQueue::new(false);
    queue.inner().lock().insert(
        1,
        RecordingStream {
            state: state.clone(),
            self_wake: true,
        },
    );
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut queue).poll_next(&mut cx).is_pending());
    assert!(Pin::new(&mut queue).poll_next(&mut cx).is_pending());

    let state = state.lock();
    assert_eq!(state.wakers.len(), 2);
    assert!(state.wakers[0].will_wake(&state.wakers[1]));
}

#[test]
fn retained_transport_waker_does_not_keep_queue_alive() {
    let state = Arc::new(Mutex::new(StreamState::default()));
    let mut queue = FairQueue::new(false);
    queue.inner().lock().insert(
        1,
        RecordingStream {
            state: state.clone(),
            self_wake: false,
        },
    );
    let inner = Arc::downgrade(&queue.inner());
    let waker = noop_waker();
    assert!(Pin::new(&mut queue)
        .poll_next(&mut Context::from_waker(&waker))
        .is_pending());

    drop(queue);

    assert!(inner.upgrade().is_none());
    state.lock().wakers[0].wake_by_ref();
}

#[test]
fn old_waker_preserves_progress_after_same_key_replacement() {
    let old = Arc::new(Mutex::new(StreamState::default()));
    let replacement = Arc::new(Mutex::new(StreamState::default()));
    let mut queue = FairQueue::new(false);
    queue.inner().lock().insert(
        1,
        RecordingStream {
            state: old.clone(),
            self_wake: false,
        },
    );
    let parent = noop_waker();
    let mut cx = Context::from_waker(&parent);
    assert!(Pin::new(&mut queue).poll_next(&mut cx).is_pending());
    queue.inner().lock().insert(
        1,
        RecordingStream {
            state: replacement.clone(),
            self_wake: false,
        },
    );
    assert!(Pin::new(&mut queue).poll_next(&mut cx).is_pending());
    replacement.lock().message = Some(42);

    // Existing routing is by key: an old transport wake can schedule the replacement.
    old.lock().wakers[0].wake_by_ref();

    assert_eq!(
        Pin::new(&mut queue).poll_next(&mut cx),
        Poll::Ready(Some((1, 42)))
    );
}

#[test]
fn stale_wakes_after_remove_or_clear_do_not_produce_messages() {
    for clear in [false, true] {
        let state = Arc::new(Mutex::new(StreamState::default()));
        let mut queue = FairQueue::new(false);
        queue.inner().lock().insert(
            1,
            RecordingStream {
                state: state.clone(),
                self_wake: false,
            },
        );
        let parent = noop_waker();
        let mut cx = Context::from_waker(&parent);
        assert!(Pin::new(&mut queue).poll_next(&mut cx).is_pending());
        if clear {
            queue.inner().lock().clear();
        } else {
            queue.inner().lock().remove(&1);
        }

        state.lock().wakers[0].wake_by_ref();

        assert_eq!(Pin::new(&mut queue).poll_next(&mut cx), Poll::Ready(None));
    }
}

#[derive(Default)]
struct ParentWakeCount(AtomicUsize);

impl ArcWake for ParentWakeCount {
    fn wake_by_ref(this: &Arc<Self>) {
        this.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn transport_wake_notifies_the_latest_parent_after_pending() {
    let state = Arc::new(Mutex::new(StreamState::default()));
    let mut queue = FairQueue::new(false);
    queue.inner().lock().insert(
        1,
        RecordingStream {
            state: state.clone(),
            self_wake: false,
        },
    );
    let first = Arc::new(ParentWakeCount::default());
    let second = Arc::new(ParentWakeCount::default());
    assert!(Pin::new(&mut queue)
        .poll_next(&mut Context::from_waker(&waker(first.clone())))
        .is_pending());
    assert!(Pin::new(&mut queue)
        .poll_next(&mut Context::from_waker(&waker(second.clone())))
        .is_pending());

    state.lock().wakers[0].wake_by_ref();

    assert_eq!(first.0.load(Ordering::Relaxed), 0);
    assert_eq!(second.0.load(Ordering::Relaxed), 1);
}
