use super::*;

const LIMIT: usize = 1024;

fn limited_codec() -> ZmqCodec {
    let mut options = SocketOptions::default();
    options.max_message_size(LIMIT);
    let mut codec = ZmqCodec::with_options(&options);
    codec.state = DecoderState::FrameHeader;
    codec.waiting_for = 1;
    codec
}

fn long_frame_header(flags: u8, len: u64) -> BytesMut {
    let mut input = BytesMut::from(&[flags | 0b0000_0010][..]);
    input.extend_from_slice(&len.to_be_bytes());
    input
}

fn is_too_large(result: Result<Option<Message>, CodecError>) -> bool {
    matches!(
        result,
        Err(CodecError::Decode(
            "Message exceeds the maximum message size"
        ))
    )
}

#[test]
fn an_announced_frame_over_the_limit_fails_before_its_body_is_reserved() {
    for len in [LIMIT as u64 + 1, 1 << 30, isize::MAX as u64 / 4] {
        let mut codec = limited_codec();
        let mut input = long_frame_header(0, len);
        let capacity = input.capacity();
        assert!(is_too_large(codec.decode(&mut input)));
        assert!(input.capacity() <= capacity, "reserved for {len}");
    }
}

#[test]
fn a_command_over_the_limit_fails_too() {
    let mut codec = limited_codec();
    let mut input = long_frame_header(0b0000_0100, LIMIT as u64 + 1);
    assert!(is_too_large(codec.decode(&mut input)));
}

#[test]
fn a_short_frame_over_a_smaller_limit_fails() {
    let mut options = SocketOptions::default();
    options.max_message_size(10);
    let mut codec = ZmqCodec::with_options(&options);
    codec.state = DecoderState::FrameHeader;
    codec.waiting_for = 1;
    let mut input = BytesMut::from(&[0, 11][..]);
    assert!(is_too_large(codec.decode(&mut input)));
}

#[test]
fn a_message_at_the_limit_is_decoded() {
    let mut codec = limited_codec();
    let mut input = long_frame_header(0, LIMIT as u64);
    input.resize(input.len() + LIMIT, 7);
    match codec.decode(&mut input).unwrap() {
        Some(Message::Message(m)) => assert_eq!(m.get(0).unwrap().len(), LIMIT),
        other => panic!("expected a message, got {other:?}"),
    }
}

#[test]
fn the_limit_counts_every_frame_of_a_multipart_message() {
    let half = LIMIT as u64 / 2;
    let mut codec = limited_codec();
    // Two frames of half the limit fill it; a third byte would exceed it.
    let mut input = long_frame_header(0b0000_0001, half);
    input.resize(input.len() + LIMIT / 2, 1);
    input.extend_from_slice(&long_frame_header(0b0000_0001, half));
    input.resize(input.len() + LIMIT / 2, 2);
    input.extend_from_slice(&[0, 1]);
    assert!(is_too_large(codec.decode(&mut input)));
}

#[test]
fn the_count_restarts_with_each_message() {
    let mut codec = limited_codec();
    let mut input = BytesMut::new();
    for _ in 0..3 {
        input.extend_from_slice(&long_frame_header(0, LIMIT as u64));
        input.resize(input.len() + LIMIT, 3);
    }
    for _ in 0..3 {
        assert!(matches!(
            codec.decode(&mut input).unwrap(),
            Some(Message::Message(_))
        ));
    }
}

#[test]
fn unlimited_by_default() {
    let mut codec = ZmqCodec::new();
    codec.state = DecoderState::FrameHeader;
    codec.waiting_for = 1;
    let mut input = long_frame_header(0, 1 << 20);
    assert!(codec.decode(&mut input).unwrap().is_none());
}
