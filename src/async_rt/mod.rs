//! General purpose helpers for async runtime cross-compatibility

pub mod block_on_read_till_set;
pub mod task;

#[cfg(feature = "tokio-runtime")]
extern crate tokio;
#[cfg(feature = "tokio-runtime")]
pub use tokio::{main, test};

#[cfg(feature = "async-std-runtime")]
extern crate async_std;
#[cfg(feature = "async-std-runtime")]
pub use async_std::{main, test};
