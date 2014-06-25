all:
	rustc zmq.rs

tests:
	rustc zmq.rs --test -o zmq.rs.test
	./zmq.rs.test --nocapture

clean:
	rm *.test
	rm *.so

