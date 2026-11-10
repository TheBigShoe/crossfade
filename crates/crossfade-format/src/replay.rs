#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_testkit::io::FaultySource;
    use std::io::Cursor;

    fn drain(r: &mut Replay) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 5];
        loop {
            let n = r.read(&mut buf).unwrap();
            if n == 0 {
                return out;
            }
            out.extend_from_slice(&buf[..n]);
        }
    }

    fn sniffed(src: impl MediaSource + 'static, n: usize) -> Replay {
        Replay::sniff(Box::new(src), n).unwrap().1
    }

    #[test]
    fn sniff_returns_the_head_and_replays_it_before_the_rest() {
        let data: Vec<u8> = (0..40).collect();
        let (head, mut r) = Replay::sniff(Box::new(Cursor::new(data.clone())), 12).unwrap();
        assert_eq!(head, &data[..12]);
        assert_eq!(drain(&mut r), data);
    }

    #[test]
    fn short_inputs_give_a_short_head_even_through_one_byte_reads() {
        let src = FaultySource::new(Cursor::new(vec![1u8, 2, 3])).max_read(1);
        let (head, mut r) = Replay::sniff(Box::new(src), 12).unwrap();
        assert_eq!(head, [1, 2, 3]);
        assert_eq!(drain(&mut r), [1, 2, 3]);
        let (head, mut r) = Replay::sniff(Box::new(Cursor::new(Vec::new())), 12).unwrap();
        assert!(head.is_empty());
        assert_eq!(drain(&mut r), []);
    }

    #[test]
    fn works_on_a_pipe_and_seeking_it_fails_cleanly() {
        let data: Vec<u8> = (0..30).collect();
        let pipe = FaultySource::new(Cursor::new(data.clone())).not_seekable();
        let mut r = sniffed(pipe, 12);
        assert!(!r.is_seekable());
        assert_eq!(r.byte_len(), None);
        assert!(r.seek(SeekFrom::Start(0)).is_err());
        assert!(r.seek(SeekFrom::Current(0)).is_err());
        assert_eq!(drain(&mut r), data);
    }

    #[test]
    fn seeking_a_seekable_source_is_absolute_and_forgets_the_replay() {
        let data: Vec<u8> = (0..40).collect();
        let mut r = sniffed(Cursor::new(data.clone()), 12);
        assert!(r.is_seekable());
        assert_eq!(r.byte_len(), Some(40));
        let mut two = [0u8; 2];
        assert_eq!(r.read(&mut two).unwrap(), 2);
        // Relative to the logical position (2), not the underlying one (12).
        assert_eq!(r.seek(SeekFrom::Current(3)).unwrap(), 5);
        assert_eq!(r.read(&mut two).unwrap(), 2);
        assert_eq!(two, [5, 6]);
        assert_eq!(r.seek(SeekFrom::Start(0)).unwrap(), 0);
        assert_eq!(drain(&mut r), data);
        assert_eq!(r.seek(SeekFrom::End(-4)).unwrap(), 36);
        assert_eq!(drain(&mut r), [36, 37, 38, 39]);
        assert_eq!(r.seek(SeekFrom::Current(-4)).unwrap(), 36);
        assert!(r.seek(SeekFrom::Current(-100)).is_err());
    }

    #[test]
    fn a_relative_seek_after_the_replay_is_plain() {
        let data: Vec<u8> = (0..40).collect();
        let mut r = sniffed(Cursor::new(data), 4);
        let mut buf = [0u8; 10];
        assert_eq!(r.read(&mut buf).unwrap(), 4);
        assert_eq!(r.read(&mut buf).unwrap(), 10);
        assert_eq!(r.seek(SeekFrom::Current(-10)).unwrap(), 4);
    }
}
