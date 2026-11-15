use super::*;
use std::collections::VecDeque;

struct BoundaryReader {
    input: Cursor<Vec<u8>>,
    boundaries: VecDeque<usize>,
    pending_at_boundary: bool,
    requested_sizes: Arc<Mutex<Vec<usize>>>,
}

impl AsyncRead for BoundaryReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let position = self.input.position() as usize;
        if self.boundaries.front().copied() == Some(position) {
            self.boundaries.pop_front();
            if self.pending_at_boundary {
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        }
        self.requested_sizes.lock().unwrap().push(buffer.len());
        let end = self
            .boundaries
            .front()
            .copied()
            .unwrap_or(self.input.get_ref().len());
        let count = buffer.len().min(end - position);
        Pin::new(&mut self.input).poll_read(cx, &mut buffer[..count])
    }
}

#[async_rt::test]
async fn read_buffer_recovers_after_a_receive_burst_with_pending() {
    let payload = vec![7_u8; 190];
    let frames = vec![payload.as_slice(); 1056];
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let source = BoundaryReader {
        input: Cursor::new(encoded_stream(&frames)),
        boundaries: (1024..=1056).map(|i| 64 + i * 192).collect(),
        pending_at_boundary: true,
        requested_sizes: Arc::clone(&sizes),
    };
    let mut reader = ZmqFramedRead::new(Box::new(source), Arc::new(SocketOptions::default()));
    let mut retained = Vec::new();
    while let Some(item) = reader.next().await {
        if let Message::Message(message) = item.unwrap() {
            retained.push(message);
        }
    }
    assert_eq!(retained.len(), frames.len());
    assert!(retained
        .iter()
        .all(|message| message.get(0).unwrap().as_ref() == payload.as_slice()));
    let sizes = sizes.lock().unwrap();
    // Retain shared output storage through the burst and the subsequent short reads.
    assert!(sizes.contains(&MAX_READ_CHUNK_SIZE));
    assert_eq!(*sizes.last().unwrap(), 512);
}

#[async_rt::test]
async fn read_buffer_uses_configured_initial_and_maximum_read_sizes() {
    let payload = vec![7_u8; 190];
    let frames = vec![payload.as_slice(); 1024];
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let source = RecordingReader::new(encoded_stream(&frames), Arc::clone(&sizes));
    let mut options = SocketOptions::default();
    options.read_buffer(300, 3000, 3);
    let mut reader = ZmqFramedRead::new(Box::new(source), Arc::new(options));
    let mut retained = Vec::new();
    while let Some(item) = reader.next().await {
        retained.push(item.unwrap());
    }
    assert_eq!(retained.len(), 1025);
    let sizes = sizes.lock().unwrap();
    assert_eq!(&sizes[..5], &[300, 600, 1200, 2400, 3000]);
    assert!(sizes.iter().all(|&size| size <= 3000));
}

#[test]
fn read_buffer_recovery_requires_consecutive_short_positive_reads() {
    for (reads, expected) in [
        (vec![190], 65_536),
        (vec![190, 190], 512),
        (vec![190, 32_768, 190], 65_536),
        (vec![190, 65_536, 190], 65_536),
        (vec![1, 1], 256),
        (vec![0], 65_536),
    ] {
        let mut reader = ZmqFramedRead::new(
            Box::new(Cursor::new(Vec::new())),
            Arc::new(SocketOptions::default()),
        );
        reader.read_chunk_size = MAX_READ_CHUNK_SIZE;
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        for count in reads {
            reader.inner = Box::new(Cursor::new(vec![7; count]));
            assert!(
                matches!(reader.poll_read_into_buffer(&mut cx, 1), Poll::Ready(Ok(n)) if n == count)
            );
        }
        assert_eq!(reader.read_chunk_size, expected);
    }
}

#[async_rt::test]
async fn read_buffer_recovery_does_not_oscillate_between_small_batches() {
    let payload = vec![7_u8; 190];
    let input = encoded_stream(&vec![payload.as_slice(); 4096]);
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let source = BoundaryReader {
        input: Cursor::new(input.clone()),
        boundaries: (1..=64).map(|i| 64 + i * 64 * 192).collect(),
        pending_at_boundary: false,
        requested_sizes: Arc::clone(&sizes),
    };
    let mut reader = ZmqFramedRead::new(Box::new(source), Arc::new(SocketOptions::default()));
    let mut retained = Vec::new();
    while let Some(item) = reader.next().await {
        if let Message::Message(message) = item.unwrap() {
            retained.push(message);
        }
    }
    assert_eq!(retained.len(), 4096);
    assert!(retained
        .iter()
        .all(|message| message.get(0).unwrap().as_ref() == payload.as_slice()));
    let sizes = sizes.lock().unwrap();
    assert_eq!(sizes.len(), 72);
    assert_eq!(sizes.iter().sum::<usize>(), 1_073_024);
}

struct InterruptedRead {
    error: bool,
}

impl AsyncRead for InterruptedRead {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buffer: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        // AsyncRead may touch the supplied space before Pending or an error.
        buffer.fill(0xFF);
        if self.error {
            Poll::Ready(Err(io::Error::from(ErrorKind::ConnectionReset)))
        } else {
            Poll::Pending
        }
    }
}

#[test]
fn read_buffer_pending_and_errors_preserve_buffered_data_and_recovery_state() {
    for error in [false, true] {
        let mut reader = ZmqFramedRead::new(
            Box::new(InterruptedRead { error }),
            Arc::new(SocketOptions::default()),
        );
        reader.read_chunk_size = MAX_READ_CHUNK_SIZE;
        reader.short_reads = 1;
        reader.buffer.extend_from_slice(b"partial frame");
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let result = reader.poll_read_into_buffer(&mut cx, 0);
        if error {
            assert!(
                matches!(result, Poll::Ready(Err(e)) if e.kind() == ErrorKind::ConnectionReset)
            );
        } else {
            assert!(result.is_pending());
        }
        let buffered_len = reader.pending_read_start.unwrap_or(reader.buffer.len());
        assert_eq!(&reader.buffer[..buffered_len], b"partial frame");
        assert_eq!(reader.read_chunk_size, MAX_READ_CHUNK_SIZE);
        assert_eq!(reader.short_reads, 1);
    }
}

#[async_rt::test]
async fn read_buffer_recovery_preserves_fragmented_retained_multipart_messages() {
    use asynchronous_codec::Encoder;
    let mut expected = crate::ZmqMessage::from(Bytes::from(vec![9; 338_729]));
    expected.push_back(Bytes::from_static(b"topic"));
    expected.push_back(Bytes::from(vec![3; 16_042]));
    let mut wire = BytesMut::from(ZmqGreeting::default());
    let mut codec = ZmqCodec::new();
    for _ in 0..3 {
        codec
            .encode(Message::Message(expected.clone()), &mut wire)
            .unwrap();
    }
    let mut boundaries = vec![1, 2, 9, 63, 64, 65, 66, 72, 73];
    boundaries.extend((8191..wire.len()).step_by(8191));
    let source = BoundaryReader {
        input: Cursor::new(wire.to_vec()),
        boundaries: boundaries.into(),
        pending_at_boundary: true,
        requested_sizes: Arc::new(Mutex::new(Vec::new())),
    };
    let mut reader = ZmqFramedRead::new(Box::new(source), Arc::new(SocketOptions::default()));
    let mut retained = Vec::new();
    while let Some(item) = reader.next().await {
        if let Message::Message(message) = item.unwrap() {
            retained.push(message);
        }
    }
    assert_eq!(retained.len(), 3);
    assert!(retained
        .iter()
        .all(|message| message.iter().eq(expected.iter())));
}

#[async_rt::test]
async fn read_buffer_recovery_reports_truncated_frames_at_eof() {
    let mut input = encoded_stream(&[&vec![7; 338_729]]);
    input.pop();
    let mut reader = ZmqFramedRead::new(
        Box::new(Cursor::new(input)),
        Arc::new(SocketOptions::default()),
    );
    assert!(matches!(
        reader.next().await,
        Some(Ok(Message::Greeting(_)))
    ));
    assert!(
        matches!(reader.next().await, Some(Err(CodecError::Io(error))) if error.kind() == ErrorKind::UnexpectedEof)
    );
}

#[test]
fn read_buffer_uses_configured_confirmation_count_and_recovery_floor() {
    for (confirmation, reads, expected) in [
        (1, vec![190], 600),
        (3, vec![190, 190], 3000),
        (3, vec![190, 190, 190], 600),
        (3, vec![190, 1500, 190, 190], 3000),
    ] {
        let mut options = SocketOptions::default();
        options.read_buffer(300, 3000, confirmation);
        let mut reader = ZmqFramedRead::new(Box::new(Cursor::new(Vec::new())), Arc::new(options));
        reader.read_chunk_size = 3000;
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        for count in reads {
            reader.inner = Box::new(Cursor::new(vec![7; count]));
            assert!(
                matches!(reader.poll_read_into_buffer(&mut cx, 1), Poll::Ready(Ok(n)) if n == count)
            );
        }
        assert_eq!(reader.read_chunk_size, expected);
    }
}

#[test]
fn read_buffer_rejects_invalid_configuration_before_socket_construction() {
    for (initial, maximum, short_reads) in [
        (0, 65536, 2),
        (128, 0, 2),
        (129, 128, 2),
        (128, isize::MAX as usize / 2 + 1, 2),
        (128, 65536, 0),
    ] {
        assert!(std::panic::catch_unwind(|| {
            SocketOptions::default().read_buffer(initial, maximum, short_reads);
        })
        .is_err());
    }
}

#[test]
fn read_buffer_shares_configuration_but_keeps_connection_state_independent() {
    let options = Arc::new(SocketOptions::default());
    let mut first = ZmqFramedRead::new(Box::new(Cursor::new(vec![7])), Arc::clone(&options));
    let second = ZmqFramedRead::new(Box::new(Cursor::new(Vec::new())), Arc::clone(&options));
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(matches!(
        first.poll_read_into_buffer(&mut cx, 1),
        Poll::Ready(Ok(1))
    ));
    assert!(Arc::ptr_eq(&first.options, &second.options));
    assert_eq!(first.short_reads, 1);
    assert_eq!(second.short_reads, 0);
}
