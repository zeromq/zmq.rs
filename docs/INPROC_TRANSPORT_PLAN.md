# Inproc Transport Plan

This note maps the current transport boundary and records the constraints for
an eventual `inproc://` path. It is intentionally a planning artifact: work
package 7 in `PERFORMANCE_ROADMAP.md` says inproc should follow the core
boundary cleanup, so this should not introduce a public endpoint or slow
prototype before the lower layers are ready.

## Current Transport Boundary

The public socket API reaches transports through two functions:

- `transport::begin_accept(endpoint, callback)` binds an endpoint, starts an
  accept task, returns the resolved endpoint, and returns an `AcceptStopHandle`.
- `transport::connect(endpoint)` opens one outgoing connection and returns a
  `FramedIo` plus the resolved peer endpoint.

The layer above the transport assumes every connection is a byte stream wrapped
in `FramedIo`. Both bind-side accepted peers and connect-side peers then run the
normal ZMTP greeting and READY exchange before the socket backend installs the
peer. After that, send/recv is backend-owned:

- round-robin sockets send through `GenericSocketBackend::send_round_robin`;
- receiving sockets insert each peer read half into `FairQueue`;
- PUB/XPUB maintain subscriber state from subscription commands;
- SUB/XSUB replay subscriptions when a peer is connected or reconnected.

Teardown is bind-map driven. `Socket::unbind` removes the resolved endpoint from
the socket's `HashMap<Endpoint, AcceptStopHandle>` and awaits shutdown of the
accept task. `Socket::close` awaits all bound endpoints. `Drop` shuts down the
backend but is not a deterministic resource-release API, so lifecycle tests that
need endpoint reuse should use `close` or `unbind`.

`connect` is also policy-bearing. It calls `connect_forever` under the socket's
connect timeout; currently connection-refused TCP and missing IPC paths are
retryable. A future inproc "no binding yet" result should participate in that
same timeout policy rather than inventing a different public behavior.

## Inproc Registry Requirements

An inproc endpoint needs a name registry, not an OS listener:

- parse `inproc://name` only when a transport implementation exists;
- reject empty names and decide whether to enforce libzmq's 256 character name
  limit at parse time or bind/connect time;
- keep active names unique while bound;
- release the name when the bind handle is shut down, and make `close`/`unbind`
  deterministic;
- support multiple connectors per bound name for all current multi-peer socket
  types;
- return the same endpoint as the resolved endpoint, because there is no
  wildcard address to resolve;
- make connect-before-bind follow zmq.rs timeout/retry semantics if the public
  API accepts `inproc://` endpoints;
- preserve socket compatibility and peer identity behavior that is currently
  enforced by the ZMTP READY exchange.

Because zmq.rs has no public `Context`, the initial registry scope would have to
be process-wide or hidden behind an internal default context. That matches the
available API but differs from libzmq's context-scoped names. Introducing public
contexts would be an API design decision and should not be hidden inside this
transport work.

## External Behavior Constraints

libzmq documents inproc as in-process, same-context message passing with no I/O
threads involved. It treats the address after `inproc://` as an arbitrary name,
requires active bind names to be unique, and documents a maximum name length of
256 characters. Current libzmq documentation says bind/connect order has not
mattered for inproc since libzmq 4.0.

The pinned OMQ source used by the perf suite (`paddor/omq.rs` at `a46c1f7`)
uses a process-global registry keyed by name, rejects duplicate active binds,
releases the registry entry when the listener drops, exchanges peer socket type
and identity snapshots during connect/accept, and skips the ZMTP codec for
inproc messages. Its current transport-level tests reject connect without a
prior bind. That is useful implementation evidence, but zmq.rs should prefer
current libzmq behavior and its own `connect_forever` policy for a public
inproc endpoint.

Useful source references:

- libzmq inproc docs: <https://zeromq.github.io/libzmq/zmq_inproc.html>
- libzmq bind/connect docs: <https://libzmq.readthedocs.io/en/latest/zmq_bind.html>,
  <https://libzmq.readthedocs.io/en/latest/zmq_connect.html>
- OMQ inproc transport: <https://github.com/paddor/omq.rs/blob/a46c1f7/omq-tokio/src/transport/inproc.rs>,
  <https://github.com/paddor/omq.rs/blob/a46c1f7/omq-compio/src/transport/inproc.rs>

## Correctness Traps

- A byte-stream-only inproc transport would be easy to wire through `FramedIo`,
  but it would preserve codec/greeting overhead and would not make zmq.rs
  comparable with OMQ or libzmq inproc.
- A codec-less fast path cannot bypass socket compatibility, identity, monitor
  events, subscription replay, disconnect notification, or fair routing.
- Registry leaks are user-visible: an inproc name left registered after drop or
  failed accept will make later tests and applications fail with duplicate bind.
- Connect-before-bind must be specified before parser support lands. Accepting
  `inproc://` and returning immediate "not found" would be observable and could
  conflict with current libzmq behavior.
- Endpoint equality is the lifecycle key. Any normalization of names must be
  applied consistently at parse, bind, connect, unbind, and monitor-event time.
- Inproc teardown has no kernel EOF to lean on. Channel closure must notify
  receiving queues and reconnection logic just as TCP/IPC EOF does today.
- PUB/XPUB subscription state is currently learned through command frames. A
  codec-less inproc path still needs command delivery or an equivalent direct
  subscription update path.

## Staged Implementation Plan

1. Keep inproc out of the public endpoint parser until the transport can connect
   and bind successfully.
2. Finish the Sans-I/O ZMTP/core boundary work so peer setup, socket
   compatibility, READY properties, and message routing can be invoked without
   forcing bytes through `FramedIo`.
3. Add an internal inproc registry module with tests for duplicate bind,
   listener drop/rebind, connect-before-bind retry classification, multiple
   pending connectors, and accept shutdown.
4. Add `Transport::Inproc` and `Endpoint::Inproc` together with parser/display
   tests and transport dispatch. The first public slice should be functional for
   PUSH/PULL and REQ/REP, not just parseable.
5. Install inproc peers through the same backend contracts as TCP/IPC, initially
   by synthesizing the same peer metadata that READY provides.
6. Extend coverage to PUB/SUB, XPUB/XSUB, DEALER/ROUTER, multi-peer fairness,
   disconnect notification, and rebind/reconnect behavior.
7. Add inproc benchmark cases to the standard suite only after correctness
   parity tests pass and generated data remains under `target/perf-runs/`.

## Test Inventory

Existing useful coverage:

- `tests/connect.rs` covers IPC connect-before-bind retry, connect timeout,
  no-timeout delayed bind, and IPC close/rebind.
- socket unit tests cover TCP wildcard/port resolution for each socket family.
- reconnection tests cover TCP restart behavior and subscription resync.

Added in this lane:

- active TCP duplicate bind is rejected;
- active IPC duplicate bind is rejected;
- unbinding an endpoint that was never bound returns `NoSuchBind`.

Those additions exercise the bind-map and active-endpoint lifecycle invariants
that the future inproc registry must preserve.

## Prototype Decision

No inproc transport prototype was attempted in this lane. The minimal
byte-stream prototype is not a no-regret path because it would add public
surface area while preserving the exact codec overhead that work package 7 is
meant to avoid. The useful no-regret work here is the lifecycle test coverage
and the staged plan for landing inproc after the core boundary is available.
