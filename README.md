# zmq.rs - native stack of ØMQ in Rust

zmq.rs is a native implementation of ØMQ in the [Rust programming language][1]. It is still in a
very early stage of designing and development, so it is **not** supposed to be used seriously now.

## Ownership and License

The contributors are listed in AUTHORS. This project uses the MPL v2 license, see LICENSE.

zmq.rs uses the [C4.1 (Collective Code Construction Contract)][2] process for contributions.

zmq.rs uses [this style guide][3] found on Rust wiki for code style.

To report an issue, use the [zmq.rs issue tracker][4] at github.com.

## Usage

There are only very few interfaces implemented till now. Try this example as `src/hello-zmq.rs`:

```rust
extern crate zmq;

use zmq::Context;

fn main() {
    let ctx = Context::new();
    let mut s = ctx.socket(zmq::REQ);
    s.bind("tcp://127.0.0.1:8876").unwrap();
    s.connect("tcp://127.0.0.1:8877").unwrap();
    loop {
        println!(">>> {}", s.msg_recv());
    }
}
```

We recommend using [cargo](https://github.com/rust-lang/cargo) to build this program. Create a file
`Cargo.toml` with:

```toml
[package]

name = "hello-zmq"
version = "0.1.0"
authors = ["you@example.com"]

[[bin]]

name = "hello-zmq"

[dependencies.zmq]

git = "https://github.com/zeromq/zmq.rs.git"
```

Then build and run with cargo, who will automatically download and build the dependencies for you:

```bash
$ cargo build
$ ./target/hello-zmq
```

Connect to `hello-zmq` at port `8876` and `8877` with any of your favorite ØMQ client and
try it out, before we have our own `send` implemented. ;)

## Development

Under C4.1 process, you are more than welcome to help us by:

* join the discussion over anything from design to code style
* fork the repository and have your own fixes
* send us pull requests
* and even star this project `^_^`

To run the test suite:

```bash
cargo test
```

## Community

As for now it is just me (fantix). You can find me at:

* IRC @ freenode: `#zeromq`, `#rust`
* Mailing lists: [`zeromq-dev`][5], [`rust-dev`][6], [`rust-china`][7]


 [1]: http://www.rust-lang.org
 [2]: http://rfc.zeromq.org/spec:22
 [3]: https://github.com/mozilla/rust/wiki/Note-style-guide
 [4]: https://github.com/decentfox/zmq.rs/issues
 [5]: http://lists.zeromq.org/mailman/listinfo/zeromq-dev
 [6]: https://mail.mozilla.org/listinfo/rust-dev
 [7]: https://groups.google.com/forum/#!forum/rust-china

