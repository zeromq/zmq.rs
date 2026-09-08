use super::*;

#[test]
fn many_empty_frames_decode_on_a_bounded_stack() {
    const CHILD_ENV: &str = "ZEROMQ_MULTIPART_STACK_TEST_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        // Stack overflow aborts the process, so keep it isolated from the test runner.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "codec::zmq_codec::tests::multipart::many_empty_frames_decode_on_a_bounded_stack",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "decoder subprocess failed: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("multipart stack test completed"));
        return;
    }

    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let mut codec = ZmqCodec::new();
            codec.state = DecoderState::FrameHeader;
            codec.waiting_for = 1;
            let mut input = BytesMut::from([1, 0].repeat(7_000).as_slice());
            input.extend_from_slice(&[0, 0]);

            let Message::Message(message) = codec.decode(&mut input).unwrap().unwrap() else {
                panic!("expected multipart message");
            };

            assert_eq!(message.len(), 7_001);
            assert!(message.iter().all(Bytes::is_empty));
            assert!(input.is_empty());
        })
        .unwrap()
        .join()
        .unwrap();
    eprintln!("multipart stack test completed");
}

#[test]
fn fragmented_multipart_preserves_empty_frames_and_following_message() {
    let mut codec = ZmqCodec::new();
    codec.state = DecoderState::FrameHeader;
    codec.waiting_for = 1;
    let mut wire = [1, 0].repeat(7_000);
    wire.extend_from_slice(&[0, 1, b'x', 0, 1, b'y']);
    let mut input = BytesMut::new();
    let mut messages = Vec::new();

    // Odd chunks split frame headers as well as multipart boundaries.
    for chunk in wire.chunks(127) {
        input.extend_from_slice(chunk);
        while let Some(message) = codec.decode(&mut input).unwrap() {
            let Message::Message(message) = message else {
                panic!("expected multipart message");
            };
            messages.push(message);
        }
    }

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].len(), 7_001);
    assert!(messages[0].iter().take(7_000).all(Bytes::is_empty));
    assert_eq!(messages[0].get(7_000), Some(&Bytes::from_static(b"x")));
    assert_eq!(messages[1].get(0), Some(&Bytes::from_static(b"y")));
    assert_eq!(messages[1].len(), 1);
    assert!(input.is_empty());
}
