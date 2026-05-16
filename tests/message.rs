#[cfg(test)]
mod test {
    use bytes::Bytes;
    use std::collections::vec_deque::VecDeque;
    use std::convert::TryFrom;
    use zeromq::ZmqMessage;

    #[test]
    fn test_single_frame_public_behavior() {
        let m = ZmqMessage::from(Bytes::from("data"));
        assert_eq!(m.len(), 1);
        assert!(!m.is_empty());
        assert_eq!(m.get(0), Some(&Bytes::from("data")));
        assert_eq!(m.get(1), None);
        assert_eq!(
            m.iter().cloned().collect::<Vec<_>>(),
            vec![Bytes::from("data")]
        );
        assert_eq!(m.clone().into_vec(), vec![Bytes::from("data")]);
        assert_eq!(
            m.into_vecdeque().into_iter().collect::<Vec<_>>(),
            vec![Bytes::from("data")]
        );
    }

    #[test]
    fn test_single_frame_promotes_to_multipart() {
        let mut m = ZmqMessage::from(Bytes::from("body"));
        m.push_front(Bytes::from("id"));
        m.push_back(Bytes::from("tail"));

        assert_eq!(m.len(), 3);
        assert_eq!(m.get(0), Some(&Bytes::from("id")));
        assert_eq!(m.get(1), Some(&Bytes::from("body")));
        assert_eq!(m.get(2), Some(&Bytes::from("tail")));
        assert_eq!(
            m.iter().rev().cloned().collect::<Vec<_>>(),
            vec![Bytes::from("tail"), Bytes::from("body"), Bytes::from("id")]
        );
    }

    #[test]
    fn test_split_off_single_frame_boundaries() {
        let mut m = ZmqMessage::from(Bytes::from("data"));
        let data = m.split_off(0);
        assert!(m.is_empty());
        assert_eq!(data.get(0), Some(&Bytes::from("data")));

        let mut m = ZmqMessage::from(Bytes::from("data"));
        let data = m.split_off(1);
        assert_eq!(m.get(0), Some(&Bytes::from("data")));
        assert!(data.is_empty());
    }

    #[test]
    fn test_split_off() {
        let mut frames = VecDeque::with_capacity(5);
        frames.push_back(Bytes::from("id1"));
        frames.push_back(Bytes::from("id2"));
        frames.push_back(Bytes::from(""));
        frames.push_back(Bytes::from("data1"));
        frames.push_back(Bytes::from("data2"));
        let mut m = ZmqMessage::try_from(frames).unwrap();
        let data = m.split_off(3);
        assert_eq!(m.len(), 3);
        assert_eq!(m.get(0), Some(&Bytes::from("id1")));
        assert_eq!(m.get(1), Some(&Bytes::from("id2")));
        assert_eq!(m.get(2), Some(&Bytes::from("")));
        assert_eq!(data.len(), 2);
        assert_eq!(data.get(0), Some(&Bytes::from("data1")));
        assert_eq!(data.get(1), Some(&Bytes::from("data2")));
    }

    #[test]
    fn test_prepend() {
        let mut frames = VecDeque::with_capacity(2);
        frames.push_back(Bytes::from("data1"));
        frames.push_back(Bytes::from("data2"));
        let mut m = ZmqMessage::try_from(frames).unwrap();

        let mut envelope_frames = VecDeque::with_capacity(3);
        envelope_frames.push_back(Bytes::from("id1"));
        envelope_frames.push_back(Bytes::from("id2"));
        envelope_frames.push_back(Bytes::from(""));
        let envelope = ZmqMessage::try_from(envelope_frames).unwrap();

        m.prepend(&envelope);
        assert_eq!(m.len(), 5);
        assert_eq!(m.get(0), Some(&Bytes::from("id1")));
        assert_eq!(m.get(1), Some(&Bytes::from("id2")));
        assert_eq!(m.get(2), Some(&Bytes::from("")));
        assert_eq!(m.get(3), Some(&Bytes::from("data1")));
        assert_eq!(m.get(4), Some(&Bytes::from("data2")));
        assert_eq!(
            m.iter().rev().cloned().collect::<Vec<_>>(),
            vec![
                Bytes::from("data2"),
                Bytes::from("data1"),
                Bytes::from(""),
                Bytes::from("id2"),
                Bytes::from("id1"),
            ]
        );
    }
}
