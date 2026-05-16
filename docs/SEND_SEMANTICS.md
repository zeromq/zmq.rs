# Send Semantics Research

This branch evaluates whether zmq.rs can use a PUSH writer task and a bounded
socket queue without making send completion, backpressure, close, and error
behavior ambiguous.

## External contract

The libzmq contract is queue based:

- `zmq_send()` queues a message part on the socket.
- A successful send does not mean the message reached the network; it means the
  socket queued the message and 0MQ has taken responsibility for it.
- For PUSH sockets, send blocks in mute state when all downstream pipes have hit
  high water mark; messages are not discarded.
- `ZMQ_SNDHWM` is a per-peer outstanding-message limit. The documented default
  is 1000 messages, but libzmq does not promise the exact usable count.
- `zmq_close()` is asynchronous with respect to pending outbound data. Unsent
  messages are controlled by `ZMQ_LINGER`.

Primary references:

- https://libzmq.readthedocs.io/en/latest/zmq_send.html
- https://zeromq.github.io/libzmq/zmq_socket.html
- https://zeromq.github.io/libzmq/zmq_setsockopt.html
- https://zeromq.github.io/libzmq/zmq_close.html

## zmq.rs baseline

Current `origin/master` has no public `SocketSend::send` documentation. The
generic backend sends directly through `ZmqFramedWrite::send(message).await`,
which makes a successful PUSH send much closer to "the framed writer accepted
and flushed this message" than libzmq's socket-queue contract.

That stronger sync point is simple, but it serializes an application send loop
with transport writes and prevents the I/O overlap that libzmq and omq.rs rely
on for small-message throughput.

## Fast-path contract under test

For PUSH, this branch treats `send().await` success as:

> The complete message was accepted into a bounded per-peer socket queue selected
> by round-robin routing.

It deliberately does not mean:

- bytes were already written to the TCP stream
- the peer received the message
- later writer errors cannot affect queued messages

Backpressure is applied by the bounded per-peer queue. When the queue is full,
`send().await` waits. If that wait is cancelled, the round-robin peer must be
returned to the routing queue so the socket does not silently lose a peer.

Writer errors remove the peer and emit `SocketEvent::Disconnected`. Messages
already accepted by the socket queue can still be lost if the transport fails
before the writer drains them; that is consistent with a queue-based send
contract, but it must be documented rather than hidden.

Close/drop currently drains any messages already buffered in the channel because
dropping the socket clears the peer map, closes the sender side, and lets the
writer task flush the receiver. There is not yet a public linger option, so this
branch documents the current behavior as "best effort drain after close" rather
than claiming configurable libzmq parity.

## Merge bar

Before this can become a real PR, the implementation needs:

- public docs for `SocketSend::send`, PUSH backpressure, and close/drop behavior
- focused tests for queue acceptance, multipart preservation, round-robin,
  backpressure cancellation, writer failure, and close/drop draining
- a decision on whether to expose `SNDHWM`/linger options now or keep the first
  patch narrowly scoped to a documented fixed queue size
- local `fmt`, `clippy`, full tests, bench dry-run, and repeated benchmarks
  against `origin/master`, libzmq, and omq.rs

## Initial benchmark read

The semantic hardening does not appear to materially change the stable parts of
the PUSH batch-writer result. Repeated local TCP runs show the same shape as the
parent batch-writer experiment:

- 8 peers, 128 B: about 246 MiB/s
- 8 peers, 2048 B: about 2.0 GiB/s

The 8-peer, 8192 B Criterion row is not trustworthy yet. It produced severe
outliers and long stalls on both this branch and the parent batch-writer branch,
so the large-frame multi-peer behavior needs a separate queue-depth/drain-rate
investigation before making a public performance claim for that row.

## Byte-aware queue follow-up

The next experiment (`f88ce18`) keeps the same queue-acceptance send contract
but changes PUSH writer backpressure from message-count only to message-count
plus byte credits:

- at most 8192 accepted-but-unflushed messages per peer
- at most 2 MiB accepted-but-unflushed payload bytes per peer, except that one
  oversized message is always allowed through an empty queue
- credits are released after the writer flushes the batch, not when it merely
  pops messages from the channel

This preserves the small-message runway while preventing 8 KiB frames from
building an implicit 64 MiB backlog per peer. It also keeps cancellation safe:
credit is acquired before the message is put on the channel, and a cancelled
wait consumes no credit.

Validation passed locally with `RUSTC_WRAPPER=`:

- `cargo +nightly fmt --all -- --check`
- both clippy profiles from the lab validation skill
- all three full test profiles from the lab validation skill
- focused backend and `push_send_batching` tests
- locked `cargo bench --no-run`

Benchmark read:

- candidate run `push-byte-hwm-f88ce18-tcp`
- candidate/libzmq/OMQ run `push-byte-hwm-f88ce18-tcp-all`
- scaffold baseline run `baseline-9533f8c-pushbyte-tcp`
- focused long row:
  `cargo bench ... zmqrs/throughput/push_pull/tcp/peers=8/8192`

Stable candidate rows:

- 8 peers, 128 B: about 261 MB/s
- 8 peers, 2048 B: about 2.23 GB/s
- 8 peers, 8192 B: about 3.18 GiB/s on the focused longer Criterion row

The same-window all-implementation run still had one severe zmq.rs outlier at
8 peers / 8192 B, so the public claim should use repeated focused rows and call
out the noisy all-run result. This is nevertheless a stronger shape than the
message-count-only queue, because the earlier multi-second stalls did not
reproduce on the focused byte-HWM run.
