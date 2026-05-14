# Performance Roadmap

This roadmap focuses on measurement and planning: no runtime internals, socket
hot paths, or public crate APIs are changed here.

## Benchmark Contract

Run data lives under `target/perf-runs/<run-id>/` and contains:

- `manifest.json`: git SHA, tool versions, OS/CPU metadata, selected profile,
  transports, implementations, runtimes, commands, and the pinned OMQ revision.
- `results.jsonl`: normalized benchmark rows from Criterion and OMQ.
- `summary.md` and `summary.html`: ratios against `zmq.rs` Tokio for matching
  workload, transport, peer count, and message size.
- `artifacts/`: copied Criterion directories and OMQ JSONL outputs.

OMQ is cloned into `target/perf-deps/omq.rs` at the pinned revision. The source
is not vendored and is not a submodule. OMQ cargo commands run with
`--manifest-path` from a neutral working directory so local `zmq.rs` Cargo
configuration does not leak into the external benchmark build.

## Work Packages

1. Benchmark harness and baseline capture
   - Goal: keep `scripts/run_perf_suite.py` and `scripts/report_perf_suite.py`
     reproducible from a fresh checkout.
   - Expected movement: none; this establishes the oracle.
   - Correctness checks: `cargo check --all-targets`, `cargo bench --no-run`,
     smoke suite for `zmq.rs` and libzmq, and OMQ smoke when the external clone
     is available.

2. By-reference send and clone reduction
   - Goal: avoid cloning message frames on send paths where the transport can
     borrow or share immutable bytes.
   - Expected movement: codec encode, PUSH/PULL, DEALER/ROUTER, and large
     multipart throughput improve without API churn.
   - Correctness checks: multipart send/recv tests, large message tests,
     codec roundtrip tests, and cancellation probes around in-flight sends.

3. PUB/XPUB fanout state cleanup
   - Goal: remove steady-state per-subscriber queue/lock pressure. Fanout
     should check connection/subscription state when state changes, then send
     without a lock per subscriber on every message.
   - Expected movement: PUB/SUB 8- and 64-subscriber fanout closes most of the
     OMQ gap.
   - Correctness checks: subscription propagation, XPUB/XSUB compliance,
     late subscriber behavior, and slow subscriber isolation.

4. Sans-I/O ZMTP core boundary
   - Goal: separate protocol parsing/framing/state from runtime I/O so Tokio
     remains first-class while transport code gets smaller and easier to test.
   - Expected movement: indirect at first; enables safer writev, recv-copy, and
     runtime policy work.
   - Correctness checks: protocol fixture tests, greeting/mechanism tests,
     interop with libzmq, and no public socket API changes.

5. Gather-write and `writev` large-frame path
   - Goal: send multipart and large frames with vectored writes where the
     runtime/transport supports it.
   - Expected movement: large message, multipart codec, and DEALER/ROUTER
     throughput improve.
   - Correctness checks: partial write simulation, large multipart interop,
     IPC/TCP parity, and benchmark confirmation across Tokio first.

6. Recv copy reduction and cancel-safety validation
   - Goal: reduce receive-side copies while guaranteeing that canceled `recv`
     calls do not drop or corrupt queued frames.
   - Expected movement: REQ/REP latency, PUSH/PULL throughput, and codec decode
     improve; cancel-safety bugs remain blocked by tests.
   - Correctness checks: timeout/cancel loops, FairQueue regression tests,
     large receive tests, and randomized receive cancellation.

7. Inproc transport
   - Goal: add an in-process transport after the core boundaries are clear.
   - Expected movement: OMQ/libzmq inproc becomes directly comparable with
     `zmq.rs`; local actor-style use cases get a meaningful fast path.
   - Correctness checks: socket lifecycle, endpoint reuse, teardown ordering,
     and multi-peer routing.

8. Runtime policy decision
   - Goal: keep Tokio mandatory and first-class. Measure async-std and
     async-dispatcher before deciding whether to keep, deprecate, or remove
     either legacy runtime path.
   - Expected movement: none by itself; reduces maintenance drag only after the
     data supports a decision.
   - Correctness checks: runtime-specific compile gates, benchmark smoke, and a
     migration note for any removed feature.

## Agent-Sized Goals

- `/goal` Capture a standard baseline on current `origin/master` and attach the
  generated summary.
- `/goal` Port by-reference send from the OMQ/Alexei branches into a small
  candidate branch and compare only codec, PUSH/PULL, and DEALER/ROUTER.
- `/goal` Prototype lock-light PUB fanout state on a candidate branch and
  compare only PUB/SUB fanout for 1, 8, and 64 subscribers.
- `/goal` Write cancel-safety stress tests for `recv` without production
  changes.
- `/goal` Audit runtime feature coverage and produce a keep/deprecate/remove
  recommendation after one standard run per runtime.
