use std::future::Future;

#[cfg(feature = "tokio-runtime")]
use tokio::runtime::{Builder, Runtime};

pub struct BenchRuntime {
    #[cfg(feature = "tokio-runtime")]
    inner: Runtime,
}

impl BenchRuntime {
    pub fn new() -> Self {
        #[cfg(feature = "tokio-runtime")]
        {
            Self {
                inner: Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("tokio runtime"),
            }
        }

        #[cfg(all(not(feature = "tokio-runtime"), feature = "async-std-runtime"))]
        {
            Self {}
        }
    }

    #[allow(clippy::unused_self)]
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        #[cfg(feature = "tokio-runtime")]
        {
            self.inner.block_on(future)
        }

        #[cfg(all(not(feature = "tokio-runtime"), feature = "async-std-runtime"))]
        {
            async_std::task::block_on(future)
        }
    }
}

impl Default for BenchRuntime {
    fn default() -> Self {
        Self::new()
    }
}
