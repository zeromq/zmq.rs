all:
	mkdir -p target
	cargo build || rustc --out-dir=target src/zmq.rs

tests:
	RUST_LOG=debug cargo test || (rustc src/zmq.rs --test -o zmq.rs.test && RUST_LOG=debug ./zmq.rs.test --nocapture)

clean:
	rm zmq.rs.test || true
	rm -r target || true

