/// Implements a generic data structure that holds a value of type T and
/// can be instantiated without a value of type T.
/// If it is read from before it is set, it will block on read until it
/// is set.
use std::sync::Arc;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::{Mutex, MutexGuard};

#[derive(Clone)]
pub struct BlockOnReadTillSet<T> {
    value: Arc<Mutex<Option<T>>>,
    sender: Arc<Mutex<Sender<()>>>,
    receiver: Arc<Mutex<Receiver<()>>>,
}

impl<T> BlockOnReadTillSet<T> {
    pub fn new() -> Self {
        let (sender, receiver) = channel(100);
        BlockOnReadTillSet {
            value: Arc::new(Mutex::new(None)),
            sender: Arc::new(Mutex::new(sender)),
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }

    pub async fn set(&self, value: T) {
        *self.value.lock().await = Some(value);
        assert!(self.value.lock().await.is_some());
        self.sender.lock().await.send(()).await.unwrap();
    }

    pub async fn is_set(&self) -> bool {
        self.value.lock().await.is_some()
    }

    pub async fn get(&self) -> MutexGuard<'_, Option<T>> {
        let mut i = 0;
        loop {
            {
                let value = self.value.lock().await;
                if value.is_some() {
                    return value;
                }
            }
            i = i + 1;
            if i > 100 {
                panic!("BlockOnReadTillSet::get() called more than 100 times without a value being set");
            }
            let mut receiver = self.receiver.lock().await;
            receiver.recv().await.unwrap();
            // We send a message to the receiver because we've
            // consumed the message that was sent when the value
            // was set. We need to send a new message so that
            // other get calls won't stay locked up if they happened
            // to be waiting for the value to be set at the same time
            // as we were.
            let sender = self.sender.lock().await;
            sender.send(()).await.unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::runtime::Runtime;

    #[test]
    fn test_block_on_read_till_set() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let block_on_read_till_set = BlockOnReadTillSet::new();
            let mut handles = Vec::new();
            for _ in 0..10 {
                let block_on_read_till_set = block_on_read_till_set.clone();
                handles.push(tokio::spawn(async move {
                    assert_eq!(block_on_read_till_set.get().await.unwrap(), 1);
                }));
            }
            let mut handles2 = Vec::new();
            // Sleep a bit before setting the value to experience the blocking behavior
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            {
                let block_on_read_till_set = block_on_read_till_set.clone();
                handles2.push(tokio::spawn(async move {
                    block_on_read_till_set.set(1).await;
                }));
            }
            for handle in handles2 {
                handle.await.unwrap();
            }
            for handle in handles {
                handle.await.unwrap();
            }
        });
    }
}
