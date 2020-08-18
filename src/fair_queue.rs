use futures::future::Pending;
use futures::task::{ArcWake, Context, Poll, Waker};
use futures::Stream;
use futures_util::core_reexport::cmp::Ordering;
use futures_util::core_reexport::panic::PanicInfo;
use futures_util::core_reexport::sync::atomic::AtomicUsize;
use futures_util::core_reexport::sync::atomic::Ordering::Relaxed;
use std::collections::{BinaryHeap, HashMap};
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

struct QueueInner<S, K> {
    ready_queue: BinaryHeap<PriorityStream<S, K>>,
    streams: HashMap<usize, PriorityStream<S, K>>,
    waker: Option<Waker>,
}

pub struct FairQueue<S, K> {
    counter: AtomicUsize,
    inner: Arc<Mutex<QueueInner<S, K>>>,
}

struct StreamWaker<S, K> {
    inner: Arc<Mutex<QueueInner<S, K>>>,
    index: usize,
}

impl<S, K> ArcWake for StreamWaker<S, K>
where
    S: Send,
    K: Send,
{
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let mut inner = arc_self.inner.lock().unwrap();
        let s = inner
            .streams
            .remove(&arc_self.index)
            .expect("Corrupted index given to waker");
        inner.ready_queue.push(s);
        if let Some(waker) = inner.waker.take() {
            waker.wake_by_ref();
        }
    }
}

struct PriorityStream<S, K> {
    priority: usize,
    key: K,
    stream: Pin<Box<S>>,
}

impl<S, K> PartialEq for PriorityStream<S, K> {
    fn eq(&self, other: &Self) -> bool {
        self.priority.eq(&other.priority)
    }
}
impl<S, K> Eq for PriorityStream<S, K> {}

impl<S, K> PartialOrd for PriorityStream<S, K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        other.priority.partial_cmp(&self.priority)
    }
}
impl<S, K> Ord for PriorityStream<S, K> {
    fn cmp(&self, other: &Self) -> Ordering {
        other.priority.cmp(&self.priority)
    }
}

impl<S, T, K> Stream for FairQueue<S, K>
where
    T: Send,
    S: Stream<Item = T> + Send,
    K: Unpin + Clone + Send,
{
    type Item = (K, T);

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        loop {
            let mut s = {
                let mut inner = stream.inner.lock().unwrap();
                inner.waker = Some(cx.waker().clone());
                match inner.ready_queue.pop() {
                    Some(s) => s,
                    None => {
                        return if inner.streams.len() > 0 {
                            Poll::Pending
                        } else {
                            Poll::Ready(None)
                        }
                    }
                }
            };

            let waker = Arc::new(StreamWaker {
                inner: stream.inner.clone(),
                index: s.priority,
            });
            let waker_ref = futures::task::waker_ref(&waker);
            let mut cx = Context::from_waker(&waker_ref);
            let p_res = s.stream.as_mut().poll_next(&mut cx);
            match p_res {
                Poll::Ready(Some(res)) => {
                    s.priority = stream.counter.fetch_add(1, Relaxed);
                    let item = Some((s.key.clone(), res));
                    stream.inner.lock().unwrap().ready_queue.push(s);
                    return Poll::Ready(item);
                }
                Poll::Ready(None) => continue,
                Poll::Pending => {
                    stream.inner.lock().unwrap().streams.insert(s.priority, s);
                    return Poll::Pending;
                }
            }
        }
    }
}

impl<S, K> FairQueue<S, K> {
    pub fn new() -> Self {
        Self {
            counter: AtomicUsize::new(0),
            inner: Arc::new(Mutex::new(QueueInner {
                ready_queue: BinaryHeap::new(),
                streams: HashMap::new(),
                waker: None,
            })),
        }
    }

    pub fn insert(&mut self, k: K, s: S) {
        let mut inner = self.inner.lock().unwrap();
        inner.ready_queue.push(PriorityStream {
            priority: self.counter.fetch_add(1, Relaxed),
            key: k,
            stream: Box::pin(s),
        });
        match &inner.waker {
            Some(w) => {
                println!("Wake up neo!");
                w.wake_by_ref()
            }
            None => (),
        };
    }
}

#[cfg(test)]
mod test {
    use crate::fair_queue::FairQueue;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_fair_queue_ready() {
        let a = futures::stream::iter(vec!["a1", "a2", "a3"]);
        let b = futures::stream::iter(vec!["b1", "b2", "b3"]);
        let c = futures::stream::iter(vec!["c1", "c2", "c3"]);

        let mut f_queue: FairQueue<_, u64> = FairQueue::new();
        f_queue.insert(1, a);
        f_queue.insert(2, b);
        f_queue.insert(3, c);

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

    #[tokio::test]
    async fn test_fair_queue_different_size() {
        let a = futures::stream::iter(vec!["a1", "a2", "a3"]);
        let b = futures::stream::iter(vec!["b1"]);
        let c = futures::stream::iter(vec!["c1", "c2"]);

        let mut f_queue: FairQueue<_, u64> = FairQueue::new();
        f_queue.insert(1, a);
        f_queue.insert(2, b);
        f_queue.insert(3, c);

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
                (3, "c2"),
                (1, "a3")
            ]
        );
    }
}
