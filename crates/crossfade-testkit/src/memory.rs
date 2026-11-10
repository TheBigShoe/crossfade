//! In-memory `JobIo`: files are byte vectors, output appears only on commit.

#[cfg(test)]
mod tests {
    use super::MemoryIo;
    use crossfade_core::{Error, JobIo, MediaSource};
    use std::io::ErrorKind;

    fn read_all(src: &mut dyn MediaSource) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4];
        loop {
            match src.read(&mut buf)? {
                0 => return Ok(out),
                n => out.extend_from_slice(&buf[..n]),
            }
        }
    }

    #[test]
    fn inserted_files_are_readable_and_listed_sorted() {
        let mut io = MemoryIo::new();
        io.insert("b", vec![1, 2, 3]);
        io.insert("a", vec![9]);
        assert_eq!(io.paths(), ["a", "b"]);
        assert_eq!(io.get("b"), Some(vec![1, 2, 3]));
        assert_eq!(io.get("zzz"), None);
        let mut src = io.open_source("b").unwrap();
        assert_eq!(src.byte_len(), Some(3));
        assert_eq!(read_all(src.as_mut()).unwrap(), [1, 2, 3]);
    }

    #[test]
    fn missing_source_is_not_found_and_names_the_path() {
        let mut io = MemoryIo::new();
        let Err(Error::Io { kind, message }) = io.open_source("nope.wav") else {
            panic!("expected Io error");
        };
        assert_eq!(kind, ErrorKind::NotFound);
        assert!(message.contains("nope.wav"), "{message}");
    }

    #[test]
    fn sink_bytes_appear_only_on_commit() {
        let mut io = MemoryIo::new();
        let mut sink = io.create_sink("out").unwrap();
        sink.write_all(b"hello").unwrap();
        assert!(io.has_pending());
        assert_eq!(io.get("out"), None);
        assert!(io.paths().is_empty());
        io.commit("out").unwrap();
        assert!(!io.has_pending());
        assert_eq!(io.get("out"), Some(b"hello".to_vec()));
    }

    #[test]
    fn discard_drops_pending_output() {
        let mut io = MemoryIo::new();
        io.insert("out", b"old".to_vec());
        let mut sink = io.create_sink("out").unwrap();
        sink.write_all(b"new").unwrap();
        io.discard("out");
        assert!(!io.has_pending());
        assert_eq!(io.get("out"), Some(b"old".to_vec()));
    }

    #[test]
    fn commit_without_a_sink_fails() {
        let mut io = MemoryIo::new();
        assert!(matches!(io.commit("x"), Err(Error::Io { .. })));
    }

    #[test]
    fn source_faults() {
        let mut io = MemoryIo::new();
        io.insert("f", vec![7; 10]);
        io.truncate_source_at("f", 4);
        let mut src = io.open_source("f").unwrap();
        assert_eq!(read_all(src.as_mut()).unwrap(), [7; 4]);

        let mut io = MemoryIo::new();
        io.insert("f", vec![7; 10]);
        io.fail_source_at("f", 6, ErrorKind::PermissionDenied);
        let mut src = io.open_source("f").unwrap();
        let err = read_all(src.as_mut()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
    }

    #[test]
    fn sink_fault_fails_writes_past_the_budget() {
        let mut io = MemoryIo::new();
        io.fail_sink_after("out", 3, ErrorKind::StorageFull);
        let mut sink = io.create_sink("out").unwrap();
        sink.write_all(b"abc").unwrap();
        assert_eq!(
            sink.write_all(b"d").unwrap_err().kind(),
            ErrorKind::StorageFull
        );
    }
}
