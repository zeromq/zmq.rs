# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Auto-reconnection for SUB sockets when PUB peer restarts (#230)
- Socket monitoring via `monitor()` for connection/disconnection events
- libzmq conformance tests for ROUTER/DEALER and PUB/SUB (#229)
- Configurable connect timeout via `SocketOptions`, defaulting to 30 seconds

### Fixed
- Subscription resync after reconnection (#231)
- Retry IPC connects while the socket file does not exist yet
- Replace panics with proper error returns in test utilities (#228)

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
