# Benchmarks

Criterion benches live in [`benches/`](../benches/). This branch carries a
master-compatible subset of the newer benchmark suite so local results can be
compared against the performance branch without pulling in branch-only APIs.
Where useful, the suite includes side-by-side `libzmq` baselines through the
`zmq2` dev dependency.

## Performance Suite

The suite in [`scripts/run_perf_suite.py`](../scripts/run_perf_suite.py)
orchestrates existing Criterion benches, normalizes the results, and can compare
against a pinned OMQ clone.

```sh
python3 scripts/run_perf_suite.py --profile smoke --impl zmqrs,libzmq --transport tcp
python3 scripts/report_perf_suite.py target/perf-runs/<run-id>
```

OMQ is not vendored. When selected, it is cloned into
`target/perf-deps/omq.rs` at the pinned revision in
[`perf-suite.json`](../perf-suite.json), or at `--omq-rev <sha>`:

```sh
python3 scripts/run_perf_suite.py --profile smoke --impl omq --transport tcp
```

If the pinned OMQ dependencies need a newer installed toolchain than the local
default, pass it explicitly:

```sh
python3 scripts/run_perf_suite.py --profile smoke --impl omq --transport tcp --toolchain 1.94.1
```

For a decision-making local run:

```sh
python3 scripts/run_perf_suite.py --profile standard --impl zmqrs,libzmq,omq --transport tcp,ipc
```

Each run writes `manifest.json`, `results.jsonl`, `summary.md`, and
`summary.html` under `target/perf-runs/<run-id>/`. Use
`--candidate-path <path>` to compare another checkout without changing this
suite. See [`BENCHMARK_RESULTS.md`](BENCHMARK_RESULTS.md) for the standard way
to package and share runs without committing generated data.

## Running Locally

```sh
# Linux
sudo apt-get install libzmq3-dev
# macOS
brew install zeromq

cargo bench --no-run
cargo bench --bench codec -- --sample-size 10
cargo bench --bench compare_libzmq -- --sample-size 10
cargo bench --bench throughput -- --sample-size 10
```

Results land under `target/criterion/`.

Socket and codec benchmarks share these optional environment controls:

- `ZMQRS_BENCH_SAMPLE_SIZE`, default `10`
- `ZMQRS_BENCH_MEASUREMENT_MS`, default `10000`
- `ZMQRS_BENCH_WARMUP_MS`, default `2000`
- `ZMQRS_BENCH_TRANSPORTS`, optional comma-separated transport filter for
  `throughput` and `compare_libzmq`; supported values are `tcp` and `ipc`.
  Invalid values fail fast instead of silently skipping transport cases.

For TCP-only smoke checks, use:

```sh
ZMQRS_BENCH_TRANSPORTS=tcp cargo bench --bench throughput -- --test
ZMQRS_BENCH_TRANSPORTS=tcp cargo bench --bench compare_libzmq -- --test
```

## Bench Targets

The master-compatible set includes:

- `codec`: pure ZMTP codec encode/decode microbenchmarks through the hidden
  `zeromq::__bench` export. These do not create sockets or perform network I/O.
  The encode benchmark includes `ZmqMessage` cloning and destination buffer
  construction; decode includes greeting decode and input buffer construction.
- `compare_libzmq`: one-message latency-style PUB/SUB, REQ/REP, PUSH/PULL, and
  DEALER/ROUTER cases over TCP and IPC, side-by-side with libzmq through
  `zmq2`. It also hosts the native/libzmq socket-send and delivered-latency
  groups so comparison code stays in one bench target.
- `throughput`: batched pipeline throughput for PUB/SUB fanout and
  DEALER/ROUTER, plus one-way DEALER/ROUTER and PUSH/PULL receive-path
  isolation. Existing `pub_fanout/send_pressure` numbers are send-pressure
  oriented: receiver timeout paths may stop early, so those numbers must not be
  read as strict delivered throughput unless the benchmark name explicitly says
  so.
- `hotpath`: focused internal hot-path experiments. The diagnostic groups
  `message_construct`, `runtime`, `backend_primitives`, and
  `async_send_overhead` are calibration aids for interpreting send-only gaps.

The suite intentionally excludes branch-only sockets, security builders,
engine internals, and `inproc` transport. libzmq peers run on OS threads;
`zeromq` peers run on a fixed 2-worker Tokio runtime.

## Reading Results

Sender-side hot-path groups measure local send admission only. A successful
`send().await` on an optimized queued path is not a transport flush or delivery
acknowledgement.

Delivered-latency benchmarks require a receiver to observe the message. They
include runtime scheduling, blocking peer threads, transport behavior, and
receive-path overhead.

Throughput benchmarks measure a sustained batch. They are the right tool for
checking whether batching, writer queue design, and receive-path changes improve
real pipeline behavior.

Criterion throughput is computed from the bytes declared by each benchmark:

- One-way send or receive benchmarks use `msg_size`.
- Fanout delivered benchmarks use `msg_size * subscriber_count`.
- Roundtrip DEALER/ROUTER delivered benchmarks use `2 * msg_size`.
- Some historical one-message comparison groups keep the older request-payload
  convention and use `msg_size` even when a reply is sent. Compare those groups
  by latency first, not by aggregate MiB/s.

Do not use a single benchmark family as the whole performance truth. The current
suite intentionally keeps sender hot path, delivered latency, and batch
throughput separate because they answer different questions.

## Known Limits

- `compare_libzmq` has a known IPC `REQ/REP` multi-case teardown issue in the
  current benchmark harness. A single case such as
  `zmqrs/req_rep/ipc/16 --test` exits normally, while the broader
  `zmqrs/req_rep/ipc --test` filter can print all success lines and still not
  exit. Use `ZMQRS_BENCH_TRANSPORTS=tcp` for full harness smoke checks when IPC
  is not the target of the run.

## Useful Commands

Smoke compile:

```sh
cargo bench --no-run
```

Short hot-path socket-send run:

```sh
ZMQRS_BENCH_SAMPLE_SIZE=10 ZMQRS_BENCH_MEASUREMENT_MS=200 ZMQRS_BENCH_WARMUP_MS=50 \
  cargo bench --bench compare_libzmq hotpath/native_socket_send/pub/subs=1/64
```

Longer native versus libzmq socket-send comparison:

```sh
ZMQRS_BENCH_SAMPLE_SIZE=20 ZMQRS_BENCH_MEASUREMENT_MS=3000 ZMQRS_BENCH_WARMUP_MS=1000 \
  cargo bench --bench compare_libzmq hotpath/.+_socket_send
```

TCP-only throughput smoke:

```sh
ZMQRS_BENCH_TRANSPORTS=tcp ZMQRS_BENCH_SAMPLE_SIZE=10 \
ZMQRS_BENCH_MEASUREMENT_MS=300 ZMQRS_BENCH_WARMUP_MS=100 \
  cargo bench --bench throughput
```

Full TCP-only benchmark harness check without running measurements:

```sh
ZMQRS_BENCH_TRANSPORTS=tcp cargo bench --bench compare_libzmq -- --test
```

## Receive-buffer recovery

`SocketOptions::read_buffer_recovery(true)` enables one complete receive policy.
The default remains grow-only: start at 128 bytes, double full reads up to 64 KiB.
Recovery waits for two consecutive qualifying short positive reads before
shrinking; frames larger than 128 KiB reserve a full 64 KiB of headroom, including space for
the read after a retained frame is split. Single large messages may retain more
capacity. Neither policy is a message-size or total-memory limit.

```rust
use zeromq::{Socket, SocketOptions, SubSocket};

let mut options = SocketOptions::default();
options.read_buffer_recovery(true);
let socket = SubSocket::with_options(options);
```

`framed_read/retained` compares both policies on identical synthetic ZMTP bytes,
retaining up to 64 messages. It covers burst/small/burst transitions, continuous
bursts, gapped 64-message batches, single and repeated large frames, and multipart
messages. Repeated large and multipart frames include both continuous input and
per-message availability boundaries. These are deterministic `AsyncRead` models,
not TCP packet boundaries or network latency tests.

```sh
cargo bench --features bench-internals --bench framed_read -- framed_read/retained
cargo bench --features bench-internals --bench framed_read -- framed_read/steady
```

Retained iterations include reader construction and destruction. The `steady`
group primes and then reuses one reader across timed batches, including its
adaptive state and prefetched messages. Use it to assess ongoing receive work;
do not interpret a single-message construction benchmark as per-message cost on
a long-lived connection.

The retained benchmark prints separate `read_work` counters once per policy:
`prepared_bytes` is the sum of slices supplied to `poll_read` (including EOF),
and `read_calls` counts those calls. In this reader, these slices correspond to
newly initialized buffer space. Neither counter measures allocator requests,
RSS, live heap or bytes copied. Timed iterations do not collect these counters.

The real socket `throughput` and `compare_libzmq` benches select recovery at socket
construction with `ZMQRS_BENCH_READ_BUFFER_RECOVERY=true`; absent or `false` keeps
the default. This setting only affects zmq.rs sockets, so libzmq cases can remain
controls. Run policies and binaries serially after all builds finish:

```sh
ZMQRS_BENCH_READ_BUFFER_RECOVERY=false cargo bench --bench throughput
ZMQRS_BENCH_READ_BUFFER_RECOVERY=true cargo bench --bench throughput
ZMQRS_BENCH_READ_BUFFER_RECOVERY=false cargo bench --bench compare_libzmq
ZMQRS_BENCH_READ_BUFFER_RECOVERY=true cargo bench --bench compare_libzmq
```

Both receive policies reject wire frame lengths that cannot fit the reader's
frame-plus-chunk capacity before reserving memory. This converts an existing
capacity panic into a decode error; it does not make allocation failure recoverable.


The retained and steady groups also include `frame_65535_boundaries`,
`frame_65536_boundaries`, `frame_65537_boundaries`, `frame_131071_boundaries`,
`frame_131072_boundaries`, and `frame_131073_boundaries`. Each cycle contains 64
single-frame messages, with reads stopping at each message boundary and up to
64 messages retained. These cases cover both sides of the reader chunk and
headroom cutoffs. Sharing a spare chunk with a retained preceding frame can
force `BytesMut::reserve` to allocate and copy an almost-complete next frame;
realloc counters alone do not observe that copy. The headroom cutoff therefore
excludes frames of at most two chunks; it is a measured policy, not a claim of a
universally optimal allocation boundary.
