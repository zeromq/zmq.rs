use futures::future::Pending;
use futures::task::{Context, Poll, Waker};
use futures::Stream;
use futures_util::core_reexport::cmp::Ordering;
use futures_util::core_reexport::panic::PanicInfo;
use futures_util::core_reexport::sync::atomic::AtomicUsize;
use futures_util::core_reexport::sync::atomic::Ordering::Relaxed;
use std::collections::{BinaryHeap, HashMap};
use std::pin::Pin;
use std::sync::Arc;

pub struct FairQueue<S, K> {
    counter: AtomicUsize,
    ready_queue: BinaryHeap<PriorityStream<S, K>>,
    streams: HashMap<usize, Pin<Box<S>>>,
    waker: Option<Waker>,
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
    S: Stream<Item = T>,
    K: Unpin + Clone,
{
    type Item = (K, Option<T>);

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        loop {
            let mut s = match stream.ready_queue.pop() {
                Some(s) => s,
                None => {
                    stream.waker = Some(cx.waker().clone());
                    return Poll::Pending;
                }
            };

            match s.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(res)) => {
                    s.priority = stream.counter.fetch_add(1, Relaxed);
                    let item = Some((s.key.clone(), Some(res)));
                    stream.ready_queue.push(s);
                    return Poll::Ready(item);
                }
                Poll::Ready(None) => continue,
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S, K> FairQueue<S, K> {
    pub fn new() -> Self {
        Self {
            counter: AtomicUsize::new(0),
            ready_queue: BinaryHeap::new(),
            streams: HashMap::new(),
            waker: None,
        }
    }

    pub fn insert(&mut self, k: K, s: S) {
        self.ready_queue.push(PriorityStream {
            priority: self.counter.fetch_add(1, Relaxed),
            key: k,
            stream: Box::pin(s),
        });
        match &self.waker {
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

        let mut f_queue = FairQueue::new();
        f_queue.insert(a);
        f_queue.insert(b);
        f_queue.insert(c);

        let mut results = Vec::new();
        while let Some(i) = f_queue.next().await {
            results.push(i);
        }
        assert_eq!(
            results,
            vec!["a1", "b1", "c1", "a2", "b2", "c2", "a3", "b3", "c3"]
        );
    }

    #[tokio::test]
    async fn test_fair_queue_different_size() {
        let a = futures::stream::iter(vec!["a1", "a2", "a3"]);
        let b = futures::stream::iter(vec!["b1"]);
        let c = futures::stream::iter(vec!["c1", "c2"]);

        let mut f_queue = FairQueue::new();
        f_queue.insert(a);
        f_queue.insert(b);
        f_queue.insert(c);

        let mut results = Vec::new();
        while let Some(i) = f_queue.next().await {
            results.push(i);
        }
        assert_eq!(results, vec!["a1", "b1", "c1", "a2", "c2", "a3"]);
    }
}
