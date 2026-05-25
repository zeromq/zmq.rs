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
