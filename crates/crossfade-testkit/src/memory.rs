//! In-memory `JobIo`: files are byte vectors, output appears only on commit.

use crate::io::{FaultySink, FaultySource};
use crossfade_core::{Error, JobIo, MediaSink, MediaSource};
use std::collections::BTreeMap;
use std::io::{self, Cursor, ErrorKind, SeekFrom};
use std::sync::{Arc, Mutex};

type Buffer = Arc<Mutex<Cursor<Vec<u8>>>>;

/// A sink whose bytes stay shared with the `MemoryIo` that created it.
struct SharedSink(Buffer);

impl SharedSink {
    fn with<T>(&self, f: impl FnOnce(&mut Cursor<Vec<u8>>) -> T) -> T {
        // A poisoned lock only means another test thread panicked.
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl MediaSink for SharedSink {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.with(|c| MediaSink::write_all(c, buf))
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.with(|c| MediaSink::seek(c, pos))
    }

    fn finish(&mut self) -> io::Result<()> {
        self.with(MediaSink::finish)
    }
}

#[derive(Default)]
struct SourceFaults {
    fail_at: Option<(u64, ErrorKind)>,
    truncate_at: Option<u64>,
}

#[derive(Default)]
pub struct MemoryIo {
    files: BTreeMap<String, Vec<u8>>,
    pending: BTreeMap<String, Buffer>,
    source_faults: BTreeMap<String, SourceFaults>,
    sink_faults: BTreeMap<String, (u64, ErrorKind)>,
}

impl MemoryIo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: &str, bytes: Vec<u8>) {
        self.files.insert(path.to_owned(), bytes);
    }

    /// Committed files only.
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).cloned()
    }

    /// Committed paths, sorted.
    pub fn paths(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    /// A sink was created and neither committed nor discarded.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn fail_source_at(&mut self, path: &str, offset: u64, kind: ErrorKind) {
        self.source_faults
            .entry(path.to_owned())
            .or_default()
            .fail_at = Some((offset, kind));
    }

    pub fn truncate_source_at(&mut self, path: &str, n: u64) {
        self.source_faults
            .entry(path.to_owned())
            .or_default()
            .truncate_at = Some(n);
    }

    pub fn fail_sink_after(&mut self, path: &str, bytes: u64, kind: ErrorKind) {
        self.sink_faults.insert(path.to_owned(), (bytes, kind));
    }
}

fn io_error(kind: ErrorKind, message: String) -> Error {
    Error::Io { kind, message }
}

impl JobIo for MemoryIo {
    fn open_source(&mut self, path: &str) -> Result<Box<dyn MediaSource>, Error> {
        let bytes = self
            .files
            .get(path)
            .ok_or_else(|| io_error(ErrorKind::NotFound, format!("{path}: no such file")))?
            .clone();
        let mut source = FaultySource::new(Cursor::new(bytes));
        if let Some(f) = self.source_faults.get(path) {
            if let Some(n) = f.truncate_at {
                source = source.truncate_at(n);
            }
            if let Some((offset, kind)) = f.fail_at {
                source = source.fail_at(offset, kind);
            }
        }
        Ok(Box::new(source))
    }

    fn create_sink(&mut self, path: &str) -> Result<Box<dyn MediaSink>, Error> {
        let buffer = Buffer::default();
        self.pending.insert(path.to_owned(), buffer.clone());
        let mut sink = FaultySink::new(SharedSink(buffer));
        if let Some(&(bytes, kind)) = self.sink_faults.get(path) {
            sink = sink.fail_after(bytes, kind);
        }
        Ok(Box::new(sink))
    }

    fn commit(&mut self, path: &str) -> Result<(), Error> {
        let buffer = self
            .pending
            .remove(path)
            .ok_or_else(|| io_error(ErrorKind::NotFound, format!("{path}: nothing to commit")))?;
        let bytes =
            std::mem::take(&mut *buffer.lock().unwrap_or_else(|e| e.into_inner())).into_inner();
        self.files.insert(path.to_owned(), bytes);
        Ok(())
    }

    fn discard(&mut self, path: &str) {
        self.pending.remove(path);
    }
}

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
