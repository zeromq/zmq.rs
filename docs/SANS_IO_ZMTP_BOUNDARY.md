# Sans-I/O ZMTP Boundary

This note scopes work package 4 from the performance roadmap: separate ZMTP
protocol parsing, framing, and handshake state from runtime I/O while keeping
Tokio as the first-class transport path.

## Current Ownership Map

- `src/transport/mod.rs`, `src/transport/tcp.rs`, and `src/transport/ipc.rs`
  own runtime-specific stream creation. Tokio streams are split and adapted
  through `tokio_util::compat`; async-std streams are split directly.
- `src/codec/framed.rs` boxes async read/write halves as `FrameableRead` and
  `FrameableWrite`, then couples both halves to `asynchronous_codec` through
  `FramedIo`.
- `src/codec/zmq_codec.rs` owns streaming parse state: greeting bytes, frame
  flags, frame length, payload body, and multipart accumulation.
- `src/codec/greeting.rs`, `src/codec/mechanism.rs`, and
  `src/codec/command.rs` parse or serialize protocol values. Before this lane,
  command serialization also owned its command-frame prefix.
- `src/util.rs` owns connection protocol state above framing: greeting exchange,
  version negotiation, READY exchange, socket compatibility, and peer identity.
- Socket backends own post-handshake state: peer tables, fair-queue streams,
  subscription state, reconnection notifiers, and monitor events.

The current shape is correct for the public API, but protocol state and runtime
I/O are interleaved enough that writev, receive-copy reduction, and runtime
policy changes have to reason through `FramedIo` and socket backends at the same
time.

## Minimal Boundary

The minimal Sans-I/O boundary should sit below socket backends and above runtime
transports:

```text
runtime transport <-> async adapter <-> Sans-I/O ZMTP core <-> socket backend
```

The core should own only deterministic protocol work:

- Greeting encode/decode and version choice.
- Frame flag and length parsing.
- Command payload parse/encode.
- Multipart assembly and message emission.
- Handshake sequencing for Greeting and READY as a pure state machine.

Runtime adapters should own:

- Tokio, async-std, and any future runtime-specific stream types.
- Read readiness, write readiness, cancellation policy, and timeout policy.
- Buffer ownership around syscalls, including vectored writes.
- Conversion between protocol actions and `Sink`/`Stream` behavior.

Socket backends should continue to own:

- Public socket semantics.
- Peer routing, fair queues, subscription filters, reconnect policy, and
  monitor events.

## Data Crossing The Boundary

Inbound data should cross as caller-owned byte buffers, with the protocol core
consuming only the bytes that make a complete ZMTP item:

- Input: `BytesMut` or a small cursor over received bytes.
- Output: a protocol event such as greeting, command, complete message, or
  "need N more bytes".
- Payload ownership: `Bytes` slices split from the input buffer so complete
  message frames do not copy.

Outbound data should cross as owned protocol values plus an encode plan:

- Input: `ZmqGreeting`, `ZmqCommand`, or `ZmqMessage`.
- Output today: bytes appended to `BytesMut`.
- Output target: a stable frame plan containing small header bytes and borrowed
  or cloned `Bytes` payloads. Tokio adapters can turn that plan into `writev`
  without changing socket APIs.

Handshake state should cross as actions instead of I/O:

- Input: local `SocketType`, local identity option, and incoming protocol
  events.
- Output: send greeting, send READY, accept peer identity, reject incompatible
  peer, or close.

## Migration Steps

1. Keep `FramedIo` and the public socket API unchanged.
2. Extract frame header and prefix handling into a private Sans-I/O helper with
   fixture tests. This lane starts that extraction in `src/codec/zmtp_frame.rs`.
3. Move command payload encoding behind the same frame helper so command and
   data frames share one flag/length implementation.
4. Make `ZmqCodec` delegate greeting, frame, command, and multipart decisions to
   a core state object while remaining the async-codec adapter.
5. Extract greeting and READY sequencing from `util.rs` into a pure
   `ZmtpSession` state machine, then let `util.rs` drive it over `FramedIo`.
6. Add a write-plan API inside the crate. Tokio remains the first optimized
   adapter, with async-std preserving the same protocol behavior.
7. Add receive-copy tests around partial frames, multipart cancellation, and
   buffer reuse before changing receive ownership.

## Correctness Fixtures

The first fixtures are byte-level tests for ZMTP frame flags and short/long
length prefixes. Follow-on fixtures should cover:

- 64-byte greeting acceptance and rejection.
- NULL, PLAIN, and CURVE mechanism parsing.
- READY command payloads with `Socket-Type` and `Identity`.
- Multipart frames split across arbitrary input chunks.
- Bad length, bad command name, and incompatible socket type failures.
- libzmq interop for greeting/READY and large multipart frames.

## Blockers For Writev And Recv-Copy Work

- `ZmqCodec::encode` still writes each outbound message into one `BytesMut`.
  Writev needs an internal encode plan that separates frame headers from
  existing `Bytes` payloads.
- `ZmqCodec::decode` still owns multipart accumulation. Receive-copy work needs
  cancellation tests before changing how partial message state is exposed.
- Handshake sequencing is still async I/O in `util.rs`. Runtime policy work
  will be safer once a pure state machine emits protocol actions.
- `FramedIo` currently erases concrete transport halves behind trait objects.
  Tokio-specific vectored-write support may need an internal fast path while
  preserving the existing boxed adapter for other runtimes.
