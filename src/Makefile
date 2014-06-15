all:
	rustc lib.rs

tests:
	rustc lib.rs --test -o zmq.rs.test
	./zmq.rs.test --nocapture

clean:
	rm *.test
	rm *.so

