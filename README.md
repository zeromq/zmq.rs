# zmq.rs - A native Rust implementation of ZeroMQ

**DISCLAIMER: The codebase is very much a work in progress and feature incomplete. DO NOT USE IN PRODUCTION**

ZeroMQ is a high-performance asynchronous messaging library that provides many popular messaging patterns for many transport types. They look and feel like Berkeley style sockets, but are fault tolerant and easier to use. This project aims to provide a native rust alternative to the [reference implementation](https://github.com/zeromq/libzmq), and leverage Rust's async ecosystem.

## Current status
A basic ZMTP implementation is working, but is not yet fully compliant to the spec. Integration tests against the reference implementation are also missing. External APIs are still subject to change - there are no semver or stability guarantees at the moment.

### Supported transport types:
We plan to support most of the basic ZMQ sockets. The current list is as follows:
* TCP
* IPC (unix only)

### Supported socket patterns:
We plan to support most of the basic ZMQ messaging patterns. The current list is as follows:
* Request/Response
* Publish/Subscribe

## Contributing
Contributions are welcome! See our issue tracker for a list of the things we need help with.

## Useful links
* https://rfc.zeromq.org/
* https://rfc.zeromq.org/spec:23/ZMTP/
* https://rfc.zeromq.org/spec:28/REQREP/
* https://rfc.zeromq.org/spec/29/
