all:
	mkdir -p target
	cargo build || rustc --out-dir=target src/zmq.rs

tests:
	rustc src/zmq.rs --test -o zmq.rs.test
	./zmq.rs.test --nocapture

clean:
	rm zmq.rs.test
	rm -r target

