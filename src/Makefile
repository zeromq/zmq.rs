all:
	rustc lib.rs

tests:
	rustc lib.rs --test -o zmq.rs.test
	./zmq.rs.test

clean:
	rm *.test
	rm *.so

