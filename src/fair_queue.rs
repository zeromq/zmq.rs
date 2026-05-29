use futures::task::{waker_ref, ArcWake};
use futures::Stream;
use parking_lot::Mutex;

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::hash::Hash;
use std::pin::Pin;
use std::sync::atomic;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

pub(crate) struct QueueInner<S, K: Clone> {
    counter: atomic::AtomicUsize,
    ready_queue: BinaryHeap<ReadyEvent<K>>,
    queued: HashSet<K>,
    /// Holds the sole connected stream so it can be polled directly, skipping
    /// the multi-peer ready-queue bookkeeping. Promotion to `streams` is
    /// one-way: once a second peer connects the queue stays on the multi-peer
    /// path for the rest of its life, even if peers later drop back to one.
    /// Re-demoting would add churn-sensitive state transitions for no real gain.
    single_stream: Option<(K, Pin<Box<S>>)>,
    streams: HashMap<K, Pin<Box<S>>>,
    waker: Option<Waker>,
    /// Callback invoked when a stream ends (peer disconnected).
    /// Wrapped in Arc so it can be cloned and called outside the lock.
    on_disconnect: Option<Arc<dyn Fn(K) + Send + Sync>>,
}

impl<S, K: Clone + Eq + Hash> QueueInner<S, K> {
    pub fn insert(&mut self, k: K, s: S) {
        let stream = Box::pin(s);
        match self.single_stream.take() {
            Some((single_key, _single_stream)) if single_key == k => {
                self.single_stream = Some((k, stream));
            }
            Some((single_key, single_stream)) => {
                // Restore the fair-queue path once a second peer connects so the
                // single-peer fast path does not affect multi-peer polling semantics.
                self.streams.insert(single_key.clone(), single_stream);
                self.push_ready(single_key);
                self.streams.insert(k.clone(), stream);
                self.push_ready(k);
            }
            None if self.streams.is_empty() => {
                self.single_stream = Some((k, stream));
            }
            None => {
                self.streams.insert(k.clone(), stream);
                self.push_ready(k);
            }
        }
        if let Some(w) = &self.waker {
            w.wake_by_ref();
        }
    }

    pub fn remove(&mut self, k: &K) {
        if self
            .single_stream
            .as_ref()
            .is_some_and(|(single_key, _)| single_key == k)
        {
            self.single_stream = None;
            return;
        }
        self.streams.remove(k);
        self.queued.remove(k);
    }

    /// Clear all streams and the ready queue.
    ///
    /// Used during shutdown to ensure TCP connections are closed even when
    /// other components (like reconnect tasks) hold Arc references to the inner.
    pub fn clear(&mut self) {
        self.single_stream = None;
        self.streams.clear();
        self.ready_queue.clear();
        self.queued.clear();
        // Wake the waker so any pending poll_next returns
        if let Some(w) = self.waker.take() {
            w.wake();
        }
    }

    fn push_ready(&mut self, k: K) {
        if self.queued.insert(k.clone()) {
            self.ready_queue.push(ReadyEvent {
                priority: self.counter.fetch_add(1, atomic::Ordering::Relaxed),
                key: k,
            });
        }
    }
}

pub struct FairQueue<S, K: Clone> {
    block_on_no_clients: bool,
    inner: Arc<Mutex<QueueInner<S, K>>>,
}

#[derive(Clone)]
struct ReadyEvent<K: Clone> {
    priority: usize,
    key: K,
}

impl<K: Clone> PartialEq for ReadyEvent<K> {
    fn eq(&self, other: &Self) -> bool {
        self.priority.eq(&other.priority)
    }
}
impl<K: Clone> Eq for ReadyEvent<K> {}

impl<K: Clone> PartialOrd for ReadyEvent<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<K: Clone> Ord for ReadyEvent<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        other.priority.cmp(&self.priority)
    }
}

struct StreamWaker<S, K: Clone> {
    inner: Arc<Mutex<QueueInner<S, K>>>,
    event: ReadyEvent<K>,
}

impl<S, K> ArcWake for StreamWaker<S, K>
where
    S: Send,
    K: Clone + Eq + Hash + Send + Sync,
{
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let mut inner = arc_self.inner.lock();
        inner.push_ready(arc_self.event.key.clone());
        if let Some(waker) = inner.waker.take() {
            waker.wake_by_ref();
        }
    }
}

impl<S, T, K> Stream for FairQueue<S, K>
where
    T: Send,
    S: Stream<Item = T> + Send + 'static,
    K: Eq + Hash + Unpin + Clone + Send + Sync + 'static,
{
    type Item = (K, T);

    #[allow(clippy::needless_continue)]
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let fair_queue = self.get_mut();
        let disconnected_single = {
            let mut inner = fair_queue.inner.lock();
            inner.waker = Some(cx.waker().clone());
            if let Some((key, stream)) = inner.single_stream.as_mut() {
                match stream.as_mut().poll_next(cx) {
                    Poll::Ready(Some(item)) => {
                        return Poll::Ready(Some((key.clone(), item)));
                    }
                    Poll::Ready(None) => {
                        let key = key.clone();
                        inner.single_stream = None;
                        Some((key, inner.on_disconnect.clone()))
                    }
                    Poll::Pending => {
                        return Poll::Pending;
                    }
                }
            } else {
                None
            }
        };
        if let Some((key, Some(callback))) = disconnected_single {
            callback(key);
        }

        let mut remaining_ready = {
            let mut inner = fair_queue.inner.lock();
            inner.waker = Some(cx.waker().clone());
            inner.ready_queue.len()
        };

        while remaining_ready > 0 {
            remaining_ready -= 1;
            let (event, mut io_stream) = {
                let mut inner = fair_queue.inner.lock();
                inner.waker = Some(cx.waker().clone());
                let event = match inner.ready_queue.pop() {
                    Some(s) => s,
                    None => return queue_empty_poll(&inner, fair_queue.block_on_no_clients),
                };
                inner.queued.remove(&event.key);
                match inner.streams.remove(&event.key) {
                    Some(stream) => (event, stream),
                    None => continue,
                }
            };

            let waker = Arc::new(StreamWaker {
                inner: fair_queue.inner.clone(),
                event: event.clone(),
            });
            let waker_ref = waker_ref(&waker);
            let mut cx = Context::from_waker(&waker_ref);
            match io_stream.as_mut().poll_next(&mut cx) {
                Poll::Ready(Some(res)) => {
                    let key = event.key.clone();
                    let item = Some((key.clone(), res));
                    let mut inner = fair_queue.inner.lock();
                    inner.streams.insert(event.key, io_stream);
                    inner.push_ready(key);
                    return Poll::Ready(item);
                }
                Poll::Ready(None) => {
                    // Peer disconnected. Don't put the stream back.
                    // Clone the callback Arc so we can call it outside the lock
                    // (to avoid deadlock if callback accesses inner)
                    let callback = {
                        let inner = fair_queue.inner.lock();
                        inner.on_disconnect.clone()
                    };
                    // Call callback outside the lock
                    if let Some(callback) = callback {
                        callback(event.key.clone());
                    }
                    // Continue to poll other streams instead of returning None immediately.
                    continue;
                }
                Poll::Pending => {
                    let mut inner = fair_queue.inner.lock();
                    inner.streams.insert(event.key, io_stream);
                    continue;
                }
            }
        }

        let mut inner = fair_queue.inner.lock();
        inner.waker = Some(cx.waker().clone());
        let should_wake = !inner.ready_queue.is_empty();
        let result = queue_empty_poll(&inner, fair_queue.block_on_no_clients);
        drop(inner);

        if should_wake {
            cx.waker().wake_by_ref();
        }
        result
    }
}

fn queue_empty_poll<S, K: Clone>(
    inner: &QueueInner<S, K>,
    block_on_no_clients: bool,
) -> Poll<Option<(K, S::Item)>>
where
    S: Stream,
{
    if inner.single_stream.is_some() || !inner.streams.is_empty() || block_on_no_clients {
        Poll::Pending
    } else {
        Poll::Ready(None)
    }
}

impl<S, K: Clone> FairQueue<S, K> {
    pub fn new(block_on_no_clients: bool) -> Self {
        Self {
            block_on_no_clients,
            inner: Arc::new(Mutex::new(QueueInner {
                counter: atomic::AtomicUsize::new(0),
                ready_queue: BinaryHeap::new(),
                queued: HashSet::new(),
                single_stream: None,
                streams: HashMap::new(),
                waker: None,
                on_disconnect: None,
            })),
        }
    }

    /// Set a callback to be invoked when a stream ends (peer disconnected).
    ///
    /// The callback receives the key of the disconnected stream.
    pub fn set_on_disconnect<F>(&mut self, callback: F)
    where
        F: Fn(K) + Send + Sync + 'static,
    {
        self.inner.lock().on_disconnect = Some(Arc::new(callback));
    }

    pub(crate) fn inner(&self) -> Arc<Mutex<QueueInner<S, K>>> {
        self.inner.clone()
    }
}

#[cfg(test)]
mod test {
    use crate::async_rt;
    use crate::fair_queue::FairQueue;
    use futures::task::noop_waker;
    use futures::{stream, Stream, StreamExt};
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};

    /// Test stream that yields Pending for the first N polls, then emits messages FIFO
    struct TestStream {
        pending_polls: usize,
        messages: VecDeque<&'static str>,
    }

    struct CountPendingStream {
        poll_count: usize,
    }

    struct WakePendingStream {
        poll_count: usize,
    }

    impl TestStream {
        fn new(pending_polls: usize, messages: &[&'static str]) -> Self {
            Self {
                pending_polls,
                messages: messages.iter().copied().collect(),
            }
        }

        fn ready(messages: &[&'static str]) -> Self {
            Self::new(0, messages)
        }

        fn pending_once(messages: &[&'static str]) -> Self {
            Self::new(1, messages)
        }
    }

    impl Stream for CountPendingStream {
        type Item = &'static str;

        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.get_mut().poll_count += 1;
            Poll::Pending
        }
    }

    impl Stream for WakePendingStream {
        type Item = &'static str;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.get_mut().poll_count += 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    impl Stream for TestStream {
        type Item = &'static str;

        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let this = self.get_mut();
            if this.pending_polls > 0 {
                this.pending_polls -= 1;
                return Poll::Pending;
            }
            Poll::Ready(this.messages.pop_front())
        }
    }

    enum UnifiedStream {
        Test(TestStream),
        CountPending(CountPendingStream),
        WakePending(WakePendingStream),
    }

    impl Stream for UnifiedStream {
        type Item = &'static str;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            match self.get_mut() {
                UnifiedStream::Test(stream) => Pin::new(stream).poll_next(cx),
                UnifiedStream::CountPending(stream) => Pin::new(stream).poll_next(cx),
                UnifiedStream::WakePending(stream) => Pin::new(stream).poll_next(cx),
            }
        }
    }

    #[async_rt::test]
    async fn test_fair_queue_ready() {
        let a = stream::iter(vec!["a1", "a2", "a3"]);
        let b = stream::iter(vec!["b1", "b2", "b3"]);
        let c = stream::iter(vec!["c1", "c2", "c3"]);

        let mut f_queue: FairQueue<_, u64> = FairQueue::new(false);
        {
            let inner = f_queue.inner();
            let mut inner_lock = inner.lock();
            inner_lock.insert(1, a);
            inner_lock.insert(2, b);
            inner_lock.insert(3, c);
        }

        let mut results = Vec::new();
        while let Some(i) = f_queue.next().await {
            results.push(i);
        }

        assert_eq!(
            results,
            vec![
                (1, "a1"),
                (2, "b1"),
                (3, "c1"),
                (1, "a2"),
                (2, "b2"),
                (3, "c2"),
                (1, "a3"),
                (2, "b3"),
                (3, "c3")
            ]
        );
    }

    #[async_rt::test]
    async fn test_fair_queue_different_size() {
        let a = stream::iter(vec!["a1", "a2", "a3"]);
        let b = stream::iter(vec!["b1"]);
        let c = stream::iter(vec!["c1", "c2"]);

        let mut f_queue: FairQueue<_, u64> = FairQueue::new(false);
        {
            let inner = f_queue.inner();
            let mut inner_lock = inner.lock();
            inner_lock.insert(1, a);
            inner_lock.insert(2, b);
            inner_lock.insert(3, c);
        }

        let mut results = Vec::new();
        while let Some(i) = f_queue.next().await {
            results.push(i);
        }

        // FairQueue continues polling all streams until all are exhausted
        assert_eq!(
            results,
            vec![
                (1, "a1"),
                (2, "b1"),
                (3, "c1"),
                (1, "a2"),
                (3, "c2"),
                (1, "a3")
            ]
        );
    }

    #[async_rt::test]
    async fn test_fair_queue_single_stream_fast_path_yields_all_items() {
        let stream = stream::iter(vec!["a1", "a2", "a3"]);
        let mut fair_queue: FairQueue<_, u64> = FairQueue::new(false);
        {
            let inner = fair_queue.inner();
            inner.lock().insert(1, stream);
        }

        let mut results = Vec::new();
        while let Some(item) = fair_queue.next().await {
            results.push(item);
        }

        assert_eq!(results, vec![(1, "a1"), (1, "a2"), (1, "a3")]);
    }

    #[async_rt::test]
    async fn test_fair_queue_promotes_single_stream_when_second_stream_is_inserted() {
        let first = stream::iter(vec!["a1", "a2"]);
        let second = stream::iter(vec!["b1", "b2"]);
        let mut fair_queue: FairQueue<_, u64> = FairQueue::new(false);
        {
            let inner = fair_queue.inner();
            let mut lock = inner.lock();
            lock.insert(1, first);
            assert!(lock.single_stream.is_some());
            assert!(lock.streams.is_empty());
            assert!(lock.ready_queue.is_empty());

            lock.insert(2, second);
            assert!(lock.single_stream.is_none());
            assert_eq!(lock.streams.len(), 2);
            assert_eq!(lock.ready_queue.len(), 2);
            assert_eq!(lock.queued.len(), 2);
        }

        let mut results = Vec::new();
        while let Some(item) = fair_queue.next().await {
            results.push(item);
        }

        assert_eq!(results, vec![(1, "a1"), (2, "b1"), (1, "a2"), (2, "b2")]);
    }

    #[async_rt::test]
    async fn test_fair_queue_single_stream_disconnect_invokes_callback() {
        let stream = stream::iter(vec!["a1"]);
        let disconnect_count = Arc::new(AtomicUsize::new(0));
        let disconnect_count_for_callback = disconnect_count.clone();
        let mut fair_queue: FairQueue<_, u64> = FairQueue::new(false);
        fair_queue.set_on_disconnect(move |peer_id| {
            assert_eq!(peer_id, 1);
            disconnect_count_for_callback.fetch_add(1, Ordering::Relaxed);
        });
        {
            let inner = fair_queue.inner();
            inner.lock().insert(1, stream);
        }

        assert_eq!(fair_queue.next().await, Some((1, "a1")));
        assert_eq!(fair_queue.next().await, None);
        assert_eq!(disconnect_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_fair_queue_promotes_single_stream_pending_across_second_insert() {
        use futures::task::{waker_ref, ArcWake};

        struct CountingWaker {
            wakes: AtomicUsize,
        }
        impl ArcWake for CountingWaker {
            fn wake_by_ref(arc_self: &Arc<Self>) {
                arc_self.wakes.fetch_add(1, Ordering::SeqCst);
            }
        }

        let counting = Arc::new(CountingWaker {
            wakes: AtomicUsize::new(0),
        });
        let waker = waker_ref(&counting);
        let mut cx = Context::from_waker(&waker);

        let mut fair_queue: FairQueue<UnifiedStream, &'static str> = FairQueue::new(false);
        {
            let inner = fair_queue.inner();
            let mut lock = inner.lock();
            lock.insert(
                "single",
                UnifiedStream::Test(TestStream::pending_once(&["a1"])),
            );
            assert!(lock.single_stream.is_some());
        }

        // The single stream is Pending on its first poll, so the queue parks on
        // the fast path rather than reporting end-of-stream.
        assert!(matches!(
            Pin::new(&mut fair_queue).poll_next(&mut cx),
            Poll::Pending
        ));

        // A second peer connects while the single stream is still pending. This
        // must move both streams onto the multi-peer path and wake the task that
        // parked on the fast path, or a real executor would never re-poll.
        {
            let inner = fair_queue.inner();
            let mut lock = inner.lock();
            lock.insert("second", UnifiedStream::Test(TestStream::ready(&["b1"])));
            assert!(lock.single_stream.is_none());
            assert_eq!(lock.streams.len(), 2);
            assert_eq!(lock.ready_queue.len(), 2);
        }
        assert!(
            counting.wakes.load(Ordering::SeqCst) >= 1,
            "second insert must wake the task parked on the single-stream fast path"
        );

        // After promotion both streams deliver via the multi-peer path.
        let mut results = Vec::new();
        for _ in 0..10 {
            match Pin::new(&mut fair_queue).poll_next(&mut cx) {
                Poll::Ready(Some(item)) => results.push(item),
                Poll::Ready(None) => break,
                Poll::Pending => {}
            }
        }
        results.sort_unstable();
        assert_eq!(results, vec![("second", "b1"), ("single", "a1")]);
    }

    #[test]
    fn test_fair_queue_continues_on_pending() {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut fair_queue: FairQueue<UnifiedStream, &str> = FairQueue::new(false);
        {
            let inner = fair_queue.inner();
            let mut lock = inner.lock();
            lock.insert(
                "slow",
                UnifiedStream::Test(TestStream::pending_once(&["s1"])),
            );
            lock.insert(
                "fast",
                UnifiedStream::Test(TestStream::ready(&["f1", "f2"])),
            );
        }

        // First poll should return fast stream (regression test: no starvation)
        let result = Pin::new(&mut fair_queue).poll_next(&mut cx);
        match result {
            Poll::Ready(Some((key, value))) => {
                assert_eq!(key, "fast");
                assert_eq!(value, "f1");
            }
            other => panic!("Expected fast stream first, got: {:#?}", other),
        }

        // Second poll: fast stream still ready, slow stream pending
        let result = Pin::new(&mut fair_queue).poll_next(&mut cx);
        match result {
            Poll::Ready(Some((key, value))) => {
                assert_eq!(key, "fast");
                assert_eq!(value, "f2");
            }
            other => panic!("Expected fast stream second, got: {:#?}", other),
        }

        // Third poll: With noop_waker, slow stream hasn't been re-polled
        let result = Pin::new(&mut fair_queue).poll_next(&mut cx);
        match result {
            Poll::Pending => {} // Expected with noop_waker
            other @ Poll::Ready(_) => panic!("Expected Pending, got: {:#?}", other),
        }
    }

    #[test]
    fn test_fair_queue_multiple_clients_fairness() {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut fair_queue: FairQueue<UnifiedStream, &str> = FairQueue::new(false);
        {
            let inner = fair_queue.inner();
            let mut lock = inner.lock();
            lock.insert(
                "fast",
                UnifiedStream::Test(TestStream::ready(&["f1", "f2", "f3"])),
            );
            lock.insert("slow", UnifiedStream::Test(TestStream::new(2, &["s1"])));
            lock.insert(
                "mid",
                UnifiedStream::Test(TestStream::new(1, &["m1", "m2"])),
            );
        }

        let mut messages = Vec::new();
        const MAX_ITERATIONS: usize = 20; // Upper bound - 3 for fast, 2 for mid, 1 for slow.

        for _ in 0..MAX_ITERATIONS {
            match Pin::new(&mut fair_queue).poll_next(&mut cx) {
                Poll::Ready(Some((key, value))) => {
                    messages.push(format!("{}:{}", key, value));

                    let has_slow = messages.iter().any(|m| m.starts_with("slow:"));
                    let fast_count = messages.iter().filter(|m| m.starts_with("fast:")).count();
                    let mid_count = messages.iter().filter(|m| m.starts_with("mid:")).count();

                    if has_slow && fast_count == 3 && mid_count == 2 {
                        break;
                    }
                }
                Poll::Ready(None) => break,
                Poll::Pending => {}
            }
        }

        // Ensure fast stream isn't starved by pending streams
        let fast_messages = messages.iter().filter(|m| m.starts_with("fast:")).count();
        assert!(
            fast_messages >= 1,
            "Fast stream was starved: {:?}",
            messages
        );
    }

    #[test]
    fn test_fair_queue_polls_ready_set_before_yielding_pending() {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut fair_queue: FairQueue<UnifiedStream, &str> = FairQueue::new(false);
        {
            let inner = fair_queue.inner();
            let mut lock = inner.lock();
            lock.insert(
                "pending-1",
                UnifiedStream::CountPending(CountPendingStream { poll_count: 0 }),
            );
            lock.insert(
                "pending-2",
                UnifiedStream::CountPending(CountPendingStream { poll_count: 0 }),
            );
            lock.insert(
                "pending-3",
                UnifiedStream::CountPending(CountPendingStream { poll_count: 0 }),
            );
        }

        let result = Pin::new(&mut fair_queue).poll_next(&mut cx);
        assert!(
            matches!(result, Poll::Pending),
            "expected FairQueue to yield Pending, got {result:?}"
        );

        let inner = fair_queue.inner();
        let mut lock = inner.lock();
        let poll_count: usize = lock
            .streams
            .values_mut()
            .map(|stream| {
                let UnifiedStream::CountPending(stream) = stream.as_mut().get_mut() else {
                    panic!("unexpected stream type");
                };
                stream.poll_count
            })
            .sum();
        assert_eq!(
            poll_count, 3,
            "FairQueue should poll all initially ready streams before yielding Pending"
        );
    }

    #[test]
    fn test_fair_queue_defers_ready_events_generated_during_poll() {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);

        let mut fair_queue: FairQueue<UnifiedStream, &str> = FairQueue::new(false);
        {
            let inner = fair_queue.inner();
            let mut lock = inner.lock();
            lock.insert(
                "pending",
                UnifiedStream::WakePending(WakePendingStream { poll_count: 0 }),
            );
            lock.insert(
                "other",
                UnifiedStream::CountPending(CountPendingStream { poll_count: 0 }),
            );
        }

        let result = Pin::new(&mut fair_queue).poll_next(&mut cx);
        assert!(
            matches!(result, Poll::Pending),
            "expected FairQueue to yield Pending, got {result:?}"
        );

        let inner = fair_queue.inner();
        let mut lock = inner.lock();
        assert_eq!(
            lock.ready_queue.len(),
            1,
            "wake generated during poll should remain queued for a later poll"
        );
        assert_eq!(
            lock.queued.len(),
            1,
            "wake generated during poll should be tracked as queued"
        );
        let stream = lock.streams.get_mut("pending").unwrap();
        let UnifiedStream::WakePending(stream) = stream.as_mut().get_mut() else {
            panic!("unexpected stream type");
        };
        assert_eq!(
            stream.poll_count, 1,
            "FairQueue should not consume same-poll wake events immediately"
        );
    }
}
