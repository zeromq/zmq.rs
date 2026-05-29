use std::future::Future;
use std::time::Duration;

use criterion::{measurement::WallTime, BenchmarkGroup};

#[allow(dead_code)]
pub const DEFAULT_TRANSPORTS: &[&str] = &["tcp", "ipc"];

// libzmq comparison parity. zmq.rs requests 4 MiB TCP send/recv buffers on
// every connection (PR #281) and runs its bench I/O on a 2-worker Tokio
// runtime, so the libzmq baseline has to be given the same buffers and io
// threads or the comparison is skewed in zmq.rs's favor.
#[allow(dead_code)]
pub const LIBZMQ_TCP_BUFFER_BYTES: i32 = 4 * 1024 * 1024;
/// Match the 2-worker bench Tokio runtime.
#[allow(dead_code)]
pub const LIBZMQ_IO_THREADS: i32 = 2;
/// Generous shared high-water mark so a small in-flight window is never the
/// bottleneck on either side. zmq.rs has no public HWM knob; its internal
/// queues are large, so libzmq gets a large HWM to match the intent.
#[allow(dead_code)]
pub const LIBZMQ_HWM: i32 = 100_000;

/// libzmq context with io threads matched to the bench Tokio worker count.
#[allow(dead_code)]
pub fn libzmq_context() -> zmq2::Context {
    let ctx = zmq2::Context::new();
    ctx.set_io_threads(LIBZMQ_IO_THREADS)
        .expect("set libzmq io_threads");
    ctx
}

/// Apply TCP buffer and HWM parity to a libzmq socket so it is tuned
/// comparably to a zmq.rs socket. The buffer size is a request that ipc
/// ignores; it only bites on tcp, which is where zmq.rs tunes too.
#[allow(dead_code)]
pub fn tune_libzmq_socket(socket: &zmq2::Socket) {
    socket
        .set_sndbuf(LIBZMQ_TCP_BUFFER_BYTES)
        .expect("set libzmq sndbuf");
    socket
        .set_rcvbuf(LIBZMQ_TCP_BUFFER_BYTES)
        .expect("set libzmq rcvbuf");
    socket.set_sndhwm(LIBZMQ_HWM).expect("set libzmq sndhwm");
    socket.set_rcvhwm(LIBZMQ_HWM).expect("set libzmq rcvhwm");
}

#[cfg(feature = "tokio-runtime")]
use tokio::runtime::{Builder, Runtime};

#[allow(dead_code)]
pub struct BenchRuntime {
    #[cfg(feature = "tokio-runtime")]
    inner: Runtime,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
pub fn selected_transports(supported: &'static [&'static str]) -> Vec<&'static str> {
    let Some(filter) = std::env::var("ZMQRS_BENCH_TRANSPORTS")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return supported.to_vec();
    };

    let selected: Vec<_> = supported
        .iter()
        .copied()
        .filter(|transport| {
            filter
                .split(',')
                .any(|candidate| candidate.trim() == *transport)
        })
        .collect();

    assert!(
        !selected.is_empty(),
        "ZMQRS_BENCH_TRANSPORTS must include at least one of: {}",
        supported.join(",")
    );
    selected
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
