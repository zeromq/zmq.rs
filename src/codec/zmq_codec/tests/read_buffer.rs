use super::*;

#[test]
fn read_buffer_recovery_keeps_a_full_chunk_after_splitting_a_large_frame() {
    for frame_len in [2 * MAX_READ_CHUNK_SIZE + 1, 338_729] {
        let mut codec = ZmqCodec::with_frame_headroom(true);
        codec.state = DecoderState::FrameHeader;
        codec.waiting_for = 1;
        let mut input = BytesMut::from(&[2][..]);
        input.extend_from_slice(&(frame_len as u64).to_be_bytes());
        assert!(codec.decode(&mut input).unwrap().is_none());
        input.resize(frame_len, 7);
        let retained = codec.decode(&mut input).unwrap().unwrap();
        assert!(input.capacity() >= MAX_READ_CHUNK_SIZE);
        let ptr = input.as_ptr();
        input.resize(MAX_READ_CHUNK_SIZE, 0);
        assert_eq!(input.as_ptr(), ptr);
        drop(retained);
    }
}

#[test]
fn read_buffer_recovery_does_not_reserve_headroom_at_the_threshold() {
    let mut codec = ZmqCodec::with_frame_headroom(true);
    codec.state = DecoderState::FrameHeader;
    codec.waiting_for = 1;
    let mut input = BytesMut::from(&[2][..]);
    input.extend_from_slice(&((2 * MAX_READ_CHUNK_SIZE) as u64).to_be_bytes());
    assert!(codec.decode(&mut input).unwrap().is_none());
    assert_eq!(input.capacity(), 2 * MAX_READ_CHUNK_SIZE);
}

#[test]
fn read_buffer_recovery_rejects_unrepresentable_wire_lengths_in_both_modes() {
    for enabled in [false, true] {
        for frame_len in [u64::MAX, isize::MAX as u64 - MAX_READ_CHUNK_SIZE as u64 + 1] {
            let mut codec = ZmqCodec::with_frame_headroom(enabled);
            codec.state = DecoderState::FrameHeader;
            codec.waiting_for = 1;
            let mut input = BytesMut::from(&[2][..]);
            input.extend_from_slice(&frame_len.to_be_bytes());
            assert!(matches!(
                codec.decode(&mut input),
                Err(CodecError::Decode(
                    "Frame length exceeds read buffer capacity"
                ))
            ));
        }
    }
}

#[test]
fn read_buffer_recovery_does_not_share_chunk_sized_frame_storage_with_the_next_read() {
    for frame_len in [
        MAX_READ_CHUNK_SIZE,
        MAX_READ_CHUNK_SIZE + 1,
        2 * MAX_READ_CHUNK_SIZE,
    ] {
        let mut codec = ZmqCodec::with_frame_headroom(true);
        codec.state = DecoderState::FrameHeader;
        codec.waiting_for = 1;
        let mut input = BytesMut::with_capacity(frame_len + 9);
        input.extend_from_slice(&[2]);
        input.extend_from_slice(&(frame_len as u64).to_be_bytes());
        assert!(codec.decode(&mut input).unwrap().is_none());
        input.resize(frame_len, 7);
        let Message::Message(retained) = codec.decode(&mut input).unwrap().unwrap() else {
            panic!("expected message");
        };
        // Sharing an entire spare chunk makes the following nearly complete
        // frame copy its body when reserve detaches from this retained frame.
        input.resize(MAX_READ_CHUNK_SIZE, 0);
        assert!(retained.get(0).unwrap().is_unique());
    }
}
