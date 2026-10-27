# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [0.7.0] - 2026-10-27

### Breaking changes
- Raise the minimum supported Rust version from 1.85 to 1.89.
- `SocketBackend::socket_options` now returns `&Arc<SocketOptions>` instead of
  `&SocketOptions`, sharing immutable options across a socket's connections.

### Added
- Runtime-neutral adoption of already-bound TCP and Unix-domain listeners via
  `Socket::bind_listener`
- Configurable adaptive receive-buffer sizing via `SocketOptions::read_buffer`.
- Performance benchmarks and libzmq comparison tooling.

### Changed
- Replace `win_uds` with an internal Windows IPC adapter using `socket2` and
  `async-io`, removing the Unlicense-only dependency while retaining Windows IPC.
- Use bounded writer queues and optimize single-peer and fan-out send paths.
  Successful sends into these queues indicate local acceptance, not delivery or
  a completed transport flush. Non-PUB queues apply backpressure; PUB retains its
  slow-subscriber drop policy.
- Enable adaptive receive-buffer recovery by default and tune Tokio TCP buffers.
- Reduce dependency features, remove `once_cell`, and update dependency versions.
- Test all three runtimes on Windows, including Rust 1.89.

### Fixed
- Preserve pending REQ replies when a receive future is cancelled.
- Guard short ZMTP command payloads and bound multipart decoder stack use.
- Avoid a fair-queue spin when pending streams wake themselves.
- Start connection retries with a short exponential backoff.
- Emit disconnect events from generic socket backends.
- Verify writer ordering across batch boundaries.

## [0.6.0] - 2026-05-04

### Added
- Auto-reconnection for SUB sockets when PUB peer restarts (#230)
- libzmq conformance tests for ROUTER/DEALER and PUB/SUB (#229)
- XSUB socket support (#236)
- `Clone` impl for `RouterSendHalf` (#235)
- IPC transport on Windows (#244)
- Configurable connect timeout via `SocketOptions`, defaulting to 30 seconds (#241)
- Monitor `Disconnected` events for DEALER, PULL, REP, ROUTER, and XPUB (#234)

### Changed
- Bumped MSRV to 1.85 and added MSRV + nightly CI checks (#239)
- PUB/XPUB/XSUB fan-out now awaits transport progress instead of dropping when a message needs multiple writes (#243)

### Fixed
- Subscription resync after reconnection (#231)
- Replace panics with proper error returns in test utilities (#228)
- Deliver large PUB/XPUB/XSUB messages that require multiple transport writes (#243)
- Retry IPC connects while the socket file does not exist yet (#241)

## [0.5.0] - 2026-02-09

### Added
- XPUB socket support (#219)
- Split trait for DealerSocket (#221) and RouterSocket (#224)
- SocketEvent::Disconnected events for PUB and SUB monitors

### Changed
- Replace dashmap with scc for better async performance (#207)
- Update to rand 0.9.x (#217)
- Update scc to 3.4.4 (#218)
- Migrate to plain `futures` crate (#202)

### Fixed
- Fair queue continues polling on disconnect instead of stopping (#207, #220)
- Propagate errors from fair queue and proxied sockets
- Improved error handling in REQ, REP, SUB, PULL sockets (#206)

## [0.4.1] - 2024-10-11

### Fixed
- Support for futures-task v0.3.31

## [0.4.0] - 2024-06-03

### Added
- async-dispatcher runtime feature (#191)

### Changed
- Updated README with runtime support status

## [0.3.5] - 2024-01-15

### Fixed
- Lint errors (#186)
- Only build IPC transport on *nix systems (#185)
- Handle codec errors in REQ socket recv (#172)

## [0.3.4] - 2023-11-01

### Changed
- Update to Rust 2021 edition (#179)
- Update asynchronous-codec and dependencies
- Replace lazy_static with once_cell
- Replace crossbeam with crossbeam-queue (#166)
- Update rand to 0.8, dashmap, parking-lot, tokio-util, uuid
- Use Bytes inside PeerIdentity (#164)
- Switch from zmq to zmq2 crate (#163)

### Fixed
- Track caller when spawning async tasks (#174)
- Only use required features of regex crate (#178)
- Automatically derive Default impl (#180)
