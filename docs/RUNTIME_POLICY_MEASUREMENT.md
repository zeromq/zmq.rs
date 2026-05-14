# Runtime Policy Measurement

Date: 2026-05-14
Branch: `quod/perf-runtime-policy-lab1`

## Goal

Keep Tokio mandatory and first-class while collecting enough evidence to decide
whether the legacy `async-std` and `async-dispatcher` runtime paths should stay,
be deprecated, or be removed later.

## Runtime Feature Map

| Runtime path | Feature set | Dependencies | Status |
| --- | --- | --- | --- |
| Tokio | `tokio-runtime,all-transport` | `tokio`, `tokio-util` | Default runtime through `default = ["tokio-runtime", "all-transport"]`. |
| async-std | `async-std-runtime,all-transport` | `async-std` | Legacy non-default runtime path. |
| async-dispatcher | `async-dispatcher-runtime,all-transport,async-dispatcher-macros` | `async-std`, `async-dispatcher`, optional macros | Legacy non-default runtime path. Uses async-std transport I/O plus async-dispatcher task/sleep/timeout helpers. |

Transport features are independent of runtime selection:
`all-transport = ["ipc-transport", "tcp-transport"]`,
`ipc-transport = ["dep:win_uds"]`, and `tcp-transport = []`.

Runtime features are de facto exclusive. The default Tokio feature set cannot
currently be combined with `async-std-runtime` because runtime-specific imports,
macros, transport adapters, and task helpers are all selected by independent
`cfg(feature = "...")` gates.

## CI Coverage Map

| Area | Tokio | async-std | async-dispatcher |
| --- | --- | --- | --- |
| Clippy/lint | Yes, default `cargo clippy --all --all-targets`. | Yes, `async-std-runtime,all-transport,async-dispatcher-macros`. | No direct async-dispatcher clippy job. |
| Tests | Yes, default `cargo test --all`. | Yes, `cargo test --all --no-default-features --features async-std-runtime,all-transport`. | Yes, `cargo test --all --no-default-features --features async-dispatcher-runtime,all-transport,async-dispatcher-macros`. |
| Perf smoke | Yes, default smoke profile uses `tokio-runtime`. | Not by default in CI. Runtime override works after the bench harness fix below. | Not by default in CI. Runtime override required a bench harness fix below. |
| Windows IPC | Yes, stable and MSRV check/test with `tokio-runtime,ipc-transport,tcp-transport`. | No. | No. |
| MSRV | Yes, default check. | Yes, async-std check. | No. |
| Nightly | Yes, default check, continue-on-error. | Yes, async-std check, continue-on-error. | No. |

The integration tests mostly use the runtime-neutral `#[async_rt::test]`
alias. `tests/push_pull_timeout.rs` is explicitly Tokio-only through
`#![cfg(feature = "tokio-runtime")]` and `#[tokio::test]`.

## Benchmark Coverage Map

| Profile or harness | Tokio | async-std | async-dispatcher | Notes |
| --- | --- | --- | --- | --- |
| `perf-suite.json` smoke defaults | Yes | No | No | Smoke declares only `["tokio-runtime"]`, but `--runtime` can override it. |
| `perf-suite.json` standard/full defaults | Yes | Yes | Yes | Config declares all three runtimes for zmq.rs. |
| `benches/compare_libzmq.rs` | Yes | Yes | Yes after harness fix | Latency-style socket comparison for PUSH/PULL, REQ/REP, DEALER/ROUTER, PUB/SUB. |
| `benches/throughput.rs` | Yes | Yes | Yes after harness fix | Pipelined DEALER/ROUTER and PUB fanout. |
| `benches/codec.rs` | Runtime-independent | Runtime-independent | Runtime-independent | Still compiled/run under selected feature sets by the perf suite. |

The benchmark runtime shim now initializes `async_dispatcher::thread_dispatcher()`
and uses `async_dispatcher::block_on()` when `async-dispatcher-runtime` is
selected. Before that fix, the async-dispatcher smoke benchmark panicked with
`The dispatcher requires a call to set_dispatcher()`.

## Command Matrix

| Command | Status | Evidence |
| --- | --- | --- |
| `cargo check --all --all-targets` | Pass | Finished `dev` profile for the default Tokio feature set. |
| `cargo check --all --all-targets --no-default-features --features tokio-runtime,all-transport` | Pass | Finished `dev` profile. |
| `cargo check --all --all-targets --no-default-features --features async-std-runtime,all-transport` | Pass | Finished `dev` profile. |
| `cargo check --all --all-targets --no-default-features --features async-dispatcher-runtime,all-transport,async-dispatcher-macros` | Pass | Finished `dev` profile. |
| `cargo test --no-run --all --no-default-features --features tokio-runtime,all-transport` | Pass | Test executables linked successfully. |
| `cargo test --no-run --all --no-default-features --features async-std-runtime,all-transport` | Pass | Test executables linked successfully. |
| `cargo test --no-run --all --no-default-features --features async-dispatcher-runtime,all-transport,async-dispatcher-macros` | Pass | Test executables linked successfully. |
| `cargo check --all-targets --features async-std-runtime` | Fail, expected | Confirms mixed default Tokio plus async-std features are unsupported; errors include duplicate runtime macros/imports and conflicting transport helpers. |
| `/home/ubuntu/labs/zmqrs-perf/with-benchmark-lock.sh -- python3 scripts/run_perf_suite.py --profile smoke --impl zmqrs --transport tcp --runtime tokio-runtime,async-std-runtime,async-dispatcher-runtime --run-id runtime-policy-smoke-runtimes --force` | Pass | `target/perf-runs/runtime-policy-smoke-runtimes/summary.md` status `complete`. |
| `cargo fmt --all -- --check` | Pass | Repository formatting is clean after applying rustfmt import ordering in `benches/codec.rs`. |

## Runtime Benchmark Smoke

Completed run:
`target/perf-runs/runtime-policy-smoke-runtimes/summary.md`

| Runtime | Workload | Transport | Size | Latency | Throughput | Ratio vs Tokio |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| Tokio | PUSH/PULL | TCP | 16 | 18.704 us | 0.9 MB/s | 1.00x |
| async-std | PUSH/PULL | TCP | 16 | 6.297 us | 2.5 MB/s | 2.97x |
| async-dispatcher | PUSH/PULL | TCP | 16 | 4.551 us | 3.5 MB/s | 4.11x |

This is a smoke result only. It proves the three runtime feature sets can run
the benchmark harness for one TCP PUSH/PULL point. It is not enough to claim a
runtime-wide performance ranking.

## Recommendation

Keep Tokio mandatory, default, and first-class.

Keep `async-std-runtime` for now. It has direct CI lint, test, MSRV, and nightly
coverage, passes the explicit compile matrix, and now has smoke benchmark
evidence. There is not enough standard/full runtime data to justify removal.

Keep `async-dispatcher-runtime` provisionally, but mark it as the higher-risk
legacy path. It passes compile and CI test coverage, and the benchmark harness
now works for smoke. However, it lacks direct clippy, MSRV, nightly, Windows IPC,
and default perf-smoke coverage. It also has global dispatcher initialization
requirements that already caused benchmark coverage to fail until fixed.

Do not remove either legacy runtime path from this branch. A later deprecation
decision should wait until standard runs complete across at least TCP and IPC
for PUSH/PULL, REQ/REP, PUB/SUB, and DEALER/ROUTER, plus throughput fanout.
If maintenance pressure forces a staged deprecation, async-dispatcher should be
evaluated first because it has more coverage gaps and more runtime-specific
setup risk than async-std.

## Migration Risks

- Users may rely on `#[zeromq::__async_rt::main]` or
  `#[zeromq::__async_rt::test]` resolving to async-std or async-dispatcher
  macros under non-default features.
- async-dispatcher users must install a dispatcher before spawning, sleeping,
  or timing out work. Removing or changing that path needs an explicit migration
  note to Tokio or async-std.
- Removing `async-std-runtime` would also affect `async-dispatcher-runtime`
  because the dispatcher feature currently depends on async-std transport I/O.
- CI currently gives Tokio the broadest platform coverage. Removing legacy
  runtimes without more measurements risks surprising downstream users who are
  green under the existing test matrix.

## Missing Measurements

- Completed standard profile per runtime. A prior TCP-only attempt left no
  completed summary artifact and is not counted as measurement evidence.
- IPC benchmark results per runtime.
- Direct CI perf-smoke coverage for async-std and async-dispatcher.
- async-dispatcher clippy, MSRV, nightly, and Windows IPC checks.
- Runtime comparisons for throughput fanout and DEALER/ROUTER under standard
  or full profile settings.
- A migration note only if a future branch actually deprecates or removes a
  runtime feature.
