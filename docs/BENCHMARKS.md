# Benchmarks

Criterion benches live in [`benches/`](../benches/). This branch carries a
master-compatible subset of the newer benchmark suite so local results can be
compared against the performance branch without pulling in branch-only APIs.

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
