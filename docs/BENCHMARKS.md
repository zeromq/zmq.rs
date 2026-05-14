# Benchmarks

Criterion benches live in [`benches/`](../benches/). This branch carries a
master-compatible subset of the newer benchmark suite so local results can be
compared against the performance branch without pulling in branch-only APIs.

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

## Bench Shape

The master-compatible set includes:

- `codec`: encode/decode microbenchmarks through the hidden `zeromq::__bench`
  export.
- `compare_libzmq`: latency-style PUB/SUB, REQ/REP, PUSH/PULL, and
  DEALER/ROUTER cases, side-by-side with libzmq through `zmq2`.
- `throughput`: batched PUB fanout and DEALER/ROUTER throughput cases.

The suite intentionally excludes branch-only sockets, security builders,
engine internals, and `inproc` transport. libzmq peers run on OS threads;
`zeromq` peers run on a fixed 2-worker Tokio runtime.
