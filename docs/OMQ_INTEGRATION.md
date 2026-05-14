# OMQ.rs Integration Notes

Issue #240 tracks interest in pulling the useful parts of
`github.com/paddor/omq.rs` into this crate without forcing a wholesale
rewrite. OMQ is not a fork, so most pieces need to come across as narrow
architectural changes rather than file-level copies.

## Incorporated

- Fanout sends now collect eligible peers and await their writes
  concurrently. This follows the performance direction discussed in the issue:
  a slow subscriber should not make PUB, XPUB, or XSUB application fanout wait
  on every other peer sequentially.
- The FairQueue recv primitive has a cancel-safety regression test. A pending
  recv future can be dropped after polling without consuming the next message.

## Good Next Slices

- Move ZMTP framing toward a sans-I/O core that can be driven by each runtime
  backend. OMQ keeps protocol state independent from file descriptors, which
  makes tests and backend experiments much cheaper.
- Add gather-write encoding for large frames. The current codec still copies
  each payload into `BytesMut` before the writer sees it; OMQ's writev path
  avoids that copy for large messages.
- Add a bounded outbound queue abstraction with explicit block/drop-newest/drop-
  oldest behavior. OMQ's routing queues make mute behavior local and testable.
- Keep runtime-backend experiments behind the existing runtime feature boundary.
  A compio or monoio backend should not change the public socket API.
- Port feature work as isolated PRs: inproc transport, PLAIN/CURVE/ZAP,
  additional draft socket types, and broader compatibility tests.

## Code-Use Boundary

OMQ is licensed ISC while this crate is MIT. Prefer implementing ideas from the
architecture and performance notes with fresh code unless a future change
explicitly carries the right attribution and license review.
