use std::future::Future;
use std::time::Duration;

use criterion::{measurement::WallTime, BenchmarkGroup};

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

        #[cfg(all(
            not(feature = "tokio-runtime"),
            any(feature = "async-std-runtime", feature = "async-dispatcher-runtime")
        ))]
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

        #[cfg(all(
            not(feature = "tokio-runtime"),
            any(feature = "async-std-runtime", feature = "async-dispatcher-runtime")
        ))]
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

pub fn configure_group(group: &mut BenchmarkGroup<'_, WallTime>) {
    group.sample_size(env_usize("ZMQRS_BENCH_SAMPLE_SIZE", 10));
    group.measurement_time(Duration::from_millis(env_u64(
        "ZMQRS_BENCH_MEASUREMENT_MS",
        10_000,
    )));
    group.warm_up_time(Duration::from_millis(env_u64(
        "ZMQRS_BENCH_WARMUP_MS",
        2_000,
    )));
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}
