#[cfg(test)]
mod tests {
    use crate::{MediaSink, MediaSource};
    use std::fs::File;
    use std::io::{Cursor, SeekFrom};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Unique temp path, removed on drop even if the test fails.
    struct TempPath(PathBuf);

    impl TempPath {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let name = format!(
                "crossfade-core-io-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            );
            Self(std::env::temp_dir().join(name))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn read_all(src: &mut dyn MediaSource) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 3];
        loop {
            match src.read(&mut buf).unwrap() {
                0 => return out,
                n => out.extend_from_slice(&buf[..n]),
            }
        }
    }

    #[test]
    fn vec_cursor_source() {
        let mut s = Cursor::new(b"hello world".to_vec());
        assert_eq!(s.byte_len(), Some(11));
        assert!(s.is_seekable());
        assert_eq!(s.seek(SeekFrom::Start(6)).unwrap(), 6);
        assert_eq!(read_all(&mut s), b"world");
    }

    #[test]
    fn slice_cursor_source() {
        let data = [1u8, 2, 3, 4];
        let mut s = Cursor::new(&data[..]);
        assert_eq!(s.byte_len(), Some(4));
        assert!(s.is_seekable());
        assert_eq!(s.seek(SeekFrom::End(-2)).unwrap(), 2);
        assert_eq!(read_all(&mut s), [3, 4]);
    }

    #[test]
    fn file_source() {
        let tmp = TempPath::new();
        std::fs::write(tmp.path(), b"0123456789").unwrap();
        let mut s = File::open(tmp.path()).unwrap();
        assert_eq!(MediaSource::byte_len(&s), Some(10));
        assert!(MediaSource::is_seekable(&s));
        MediaSource::seek(&mut s, SeekFrom::Start(7)).unwrap();
        assert_eq!(read_all(&mut s), b"789");
    }

    #[test]
    fn source_is_object_safe() {
        let mut boxed: Box<dyn MediaSource> = Box::new(Cursor::new(vec![9u8, 8, 7]));
        assert_eq!(boxed.byte_len(), Some(3));
        assert_eq!(read_all(boxed.as_mut()), [9, 8, 7]);
    }

    #[test]
    fn vec_cursor_sink() {
        let mut s = Cursor::new(Vec::new());
        s.write_all(b"abcdef").unwrap();
        s.seek(SeekFrom::Start(1)).unwrap();
        s.write_all(b"XY").unwrap();
        s.finish().unwrap();
        assert_eq!(s.into_inner(), b"aXYdef");
    }

    #[test]
    fn file_sink() {
        let tmp = TempPath::new();
        let mut s = File::create(tmp.path()).unwrap();
        MediaSink::write_all(&mut s, b"head-body").unwrap();
        MediaSink::seek(&mut s, SeekFrom::Start(0)).unwrap();
        MediaSink::write_all(&mut s, b"HEAD").unwrap();
        MediaSink::finish(&mut s).unwrap();
        assert_eq!(std::fs::read(tmp.path()).unwrap(), b"HEAD-body");
    }

    #[test]
    fn sink_is_object_safe() {
        let mut boxed: Box<dyn MediaSink> = Box::new(Cursor::new(Vec::new()));
        boxed.write_all(b"ok").unwrap();
        boxed.finish().unwrap();
    }
}
