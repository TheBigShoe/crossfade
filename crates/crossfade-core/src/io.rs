use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

/// Blocking byte input. Not `Read + Seek` so a non-seekable source (a pipe)
/// can say so and have `seek` fail instead of lying.
pub trait MediaSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64>;
    fn byte_len(&self) -> Option<u64>;
    fn is_seekable(&self) -> bool;
}

/// Blocking byte output. `finish` flushes and surfaces deferred errors.
pub trait MediaSink {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()>;
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64>;
    fn finish(&mut self) -> io::Result<()>;
}

impl MediaSource for Cursor<Vec<u8>> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Read::read(self, buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        Seek::seek(self, pos)
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.get_ref().len() as u64)
    }

    fn is_seekable(&self) -> bool {
        true
    }
}

impl MediaSource for Cursor<&[u8]> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Read::read(self, buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        Seek::seek(self, pos)
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.get_ref().len() as u64)
    }

    fn is_seekable(&self) -> bool {
        true
    }
}

impl MediaSource for File {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        Read::read(self, buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        Seek::seek(self, pos)
    }

    fn byte_len(&self) -> Option<u64> {
        self.metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|m| m.len())
    }

    // Devices and pipes opened as `File` report no length and cannot seek.
    fn is_seekable(&self) -> bool {
        self.metadata().is_ok_and(|m| m.is_file())
    }
}

impl MediaSink for Cursor<Vec<u8>> {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        Write::write_all(self, buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        Seek::seek(self, pos)
    }

    fn finish(&mut self) -> io::Result<()> {
        Write::flush(self)
    }
}

impl MediaSink for File {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        Write::write_all(self, buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        Seek::seek(self, pos)
    }

    fn finish(&mut self) -> io::Result<()> {
        Write::flush(self)?;
        self.sync_all()
    }
}

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
        assert_eq!(MediaSource::seek(&mut s, SeekFrom::Start(6)).unwrap(), 6);
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
        MediaSink::seek(&mut s, SeekFrom::Start(1)).unwrap();
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

    struct OneFile(Vec<String>);

    impl crate::JobIo for OneFile {
        fn open_source(&mut self, path: &str) -> Result<Box<dyn MediaSource>, crate::Error> {
            self.0.push(format!("open {path}"));
            Ok(Box::new(Cursor::new(vec![1u8])))
        }
        fn create_sink(&mut self, path: &str) -> Result<Box<dyn MediaSink>, crate::Error> {
            self.0.push(format!("create {path}"));
            Ok(Box::new(Cursor::new(Vec::new())))
        }
        fn commit(&mut self, path: &str) -> Result<(), crate::Error> {
            self.0.push(format!("commit {path}"));
            Ok(())
        }
        fn discard(&mut self, path: &str) {
            self.0.push(format!("discard {path}"));
        }
    }

    #[test]
    fn job_io_is_object_safe() {
        let mut io = OneFile(Vec::new());
        let dynamic: &mut dyn crate::JobIo = &mut io;
        assert_eq!(dynamic.open_source("a").unwrap().byte_len(), Some(1));
        dynamic.create_sink("b").unwrap();
        dynamic.commit("b").unwrap();
        dynamic.discard("b");
        assert_eq!(io.0, ["open a", "create b", "commit b", "discard b"]);
    }

    #[test]
    fn sink_is_object_safe() {
        let mut boxed: Box<dyn MediaSink> = Box::new(Cursor::new(Vec::new()));
        boxed.write_all(b"ok").unwrap();
        boxed.finish().unwrap();
    }
}
