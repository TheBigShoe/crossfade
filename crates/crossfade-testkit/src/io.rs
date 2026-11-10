//! Fakes for the `MediaSource` / `MediaSink` seams.

use crossfade_core::{MediaSink, MediaSource};
use std::io::{self, ErrorKind, SeekFrom};

/// Wraps a source and misbehaves as configured. Options combine.
pub struct FaultySource<S: MediaSource> {
    inner: S,
    truncate: Option<u64>,
    max_read: Option<usize>,
    fail: Option<(u64, ErrorKind)>,
    seekable: bool,
    pos: u64,
}

impl<S: MediaSource> FaultySource<S> {
    pub fn new(mut inner: S) -> Self {
        // Track the position ourselves: faults are keyed on absolute offsets.
        let pos = inner.seek(SeekFrom::Current(0)).unwrap_or(0);
        Self {
            inner,
            truncate: None,
            max_read: None,
            fail: None,
            seekable: true,
            pos,
        }
    }

    /// Behaves as a file only `n` bytes long.
    pub fn truncate_at(mut self, n: u64) -> Self {
        self.truncate = Some(n);
        self
    }

    /// No single read returns more than `n` bytes (`n` of 0 is treated as 1).
    pub fn max_read(mut self, n: usize) -> Self {
        self.max_read = Some(n.max(1));
        self
    }

    /// Bytes before `offset` read normally; the read that would reach it
    /// returns the preceding bytes first, then every read from `offset` on
    /// fails with `kind`.
    pub fn fail_at(mut self, offset: u64, kind: ErrorKind) -> Self {
        self.fail = Some((offset, kind));
        self
    }

    /// Looks like a pipe: no length, no seeking.
    pub fn not_seekable(mut self) -> Self {
        self.seekable = false;
        self
    }
}

impl<S: MediaSource> MediaSource for FaultySource<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut limit = buf.len().min(self.max_read.unwrap_or(usize::MAX));
        let mut cap = |end: u64| {
            let room = end.saturating_sub(self.pos);
            limit = limit.min(usize::try_from(room).unwrap_or(usize::MAX));
        };
        if let Some(n) = self.truncate {
            cap(n);
        }
        if let Some((offset, kind)) = self.fail {
            if self.pos >= offset {
                return Err(io::Error::from(kind));
            }
            cap(offset);
        }
        if limit == 0 {
            return Ok(0);
        }
        let n = self.inner.read(&mut buf[..limit])?;
        self.pos += n as u64;
        Ok(n)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if !self.seekable {
            return Err(io::Error::new(
                ErrorKind::Unsupported,
                "source is not seekable",
            ));
        }
        // From-end must be relative to the truncated length.
        let target = match (pos, self.byte_len()) {
            (SeekFrom::End(d), Some(len)) => {
                let abs = len
                    .checked_add_signed(d)
                    .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "seek before start"))?;
                SeekFrom::Start(abs)
            }
            (p, _) => p,
        };
        self.pos = self.inner.seek(target)?;
        Ok(self.pos)
    }

    fn byte_len(&self) -> Option<u64> {
        if !self.seekable {
            return None;
        }
        let len = self.inner.byte_len()?;
        Some(self.truncate.map_or(len, |n| n.min(len)))
    }

    fn is_seekable(&self) -> bool {
        self.seekable && self.inner.is_seekable()
    }
}

/// Wraps a sink and fails as configured.
pub struct FaultySink<S: MediaSink> {
    inner: S,
    fail_after: Option<(u64, ErrorKind)>,
    fail_on_finish: Option<ErrorKind>,
    written: u64,
}

impl<S: MediaSink> FaultySink<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            fail_after: None,
            fail_on_finish: None,
            written: 0,
        }
    }

    /// Simulates a full disk. Writes succeed while the running total of
    /// bytes written stays within `bytes`. The write that would exceed it
    /// is partial: the bytes that still fit land in the inner sink, then
    /// it fails with `kind`, and so does every later non-empty write.
    /// Seeks do not change the count.
    pub fn fail_after(mut self, bytes: u64, kind: ErrorKind) -> Self {
        self.fail_after = Some((bytes, kind));
        self
    }

    /// `finish` fails with `kind` without finishing the inner sink.
    pub fn fail_on_finish(mut self, kind: ErrorKind) -> Self {
        self.fail_on_finish = Some(kind);
        self
    }

    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: MediaSink> MediaSink for FaultySink<S> {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        let Some((limit, kind)) = self.fail_after else {
            return self.inner.write_all(buf);
        };
        let room = limit.saturating_sub(self.written);
        let fits = usize::try_from(room).map_or(buf.len(), |r| r.min(buf.len()));
        self.inner.write_all(&buf[..fits])?;
        self.written += fits as u64;
        if fits < buf.len() {
            return Err(io::Error::from(kind));
        }
        Ok(())
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }

    fn finish(&mut self) -> io::Result<()> {
        if let Some(kind) = self.fail_on_finish {
            return Err(io::Error::from(kind));
        }
        self.inner.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const FAIL: ErrorKind = ErrorKind::PermissionDenied;

    fn data(n: u8) -> Cursor<Vec<u8>> {
        Cursor::new((0..n).collect())
    }

    /// Reads with `chunk`-sized buffers until EOF or error. Returns the
    /// bytes, the sizes of the successful reads, and the error kind if any.
    fn drain(src: &mut dyn MediaSource, chunk: usize) -> (Vec<u8>, Vec<usize>, Option<ErrorKind>) {
        let (mut out, mut sizes) = (Vec::new(), Vec::new());
        let mut buf = vec![0u8; chunk];
        loop {
            match src.read(&mut buf) {
                Ok(0) => return (out, sizes, None),
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    sizes.push(n);
                }
                Err(e) => return (out, sizes, Some(e.kind())),
            }
        }
    }

    #[test]
    fn passes_through_by_default() {
        let mut s = FaultySource::new(data(10));
        assert_eq!(s.byte_len(), Some(10));
        assert!(s.is_seekable());
        assert_eq!(drain(&mut s, 4).0, (0..10).collect::<Vec<u8>>());
    }

    #[test]
    fn truncate_cuts_reads_and_length() {
        let mut s = FaultySource::new(data(10)).truncate_at(6);
        assert_eq!(s.byte_len(), Some(6));
        assert_eq!(drain(&mut s, 4).0, [0, 1, 2, 3, 4, 5]);
        // Longer than the real source: length stays the real one.
        let s = FaultySource::new(data(10)).truncate_at(99);
        assert_eq!(s.byte_len(), Some(10));
    }

    #[test]
    fn truncate_applies_to_seek_from_end() {
        let mut s = FaultySource::new(data(10)).truncate_at(6);
        assert_eq!(s.seek(SeekFrom::End(-2)).unwrap(), 4);
        assert_eq!(drain(&mut s, 8).0, [4, 5]);
    }

    #[test]
    fn seeking_past_the_truncation_reads_nothing() {
        let mut s = FaultySource::new(data(10)).truncate_at(6);
        s.seek(SeekFrom::Start(8)).unwrap();
        assert_eq!(drain(&mut s, 8).0, []);
    }

    #[test]
    fn max_read_caps_every_read() {
        let mut s = FaultySource::new(data(10)).max_read(3);
        let (bytes, sizes, err) = drain(&mut s, 8);
        assert_eq!(bytes, (0..10).collect::<Vec<u8>>());
        assert_eq!(sizes, [3, 3, 3, 1]);
        assert_eq!(err, None);
    }

    #[test]
    fn fail_at_returns_prefix_then_errors_every_time() {
        let mut s = FaultySource::new(data(10)).fail_at(5, FAIL);
        let (bytes, sizes, err) = drain(&mut s, 4);
        assert_eq!(bytes, [0, 1, 2, 3, 4]);
        assert_eq!(sizes, [4, 1]);
        assert_eq!(err, Some(FAIL));
        let mut buf = [0u8; 2];
        assert_eq!(s.read(&mut buf).unwrap_err().kind(), FAIL);
        assert_eq!(s.read(&mut buf).unwrap_err().kind(), FAIL);
    }

    #[test]
    fn fail_at_zero_fails_immediately() {
        let mut s = FaultySource::new(data(10)).fail_at(0, ErrorKind::BrokenPipe);
        assert_eq!(drain(&mut s, 4).2, Some(ErrorKind::BrokenPipe));
    }

    #[test]
    fn seeking_back_before_the_fault_makes_reads_work_again() {
        let mut s = FaultySource::new(data(10)).fail_at(5, FAIL);
        assert_eq!(drain(&mut s, 8).2, Some(FAIL));
        s.seek(SeekFrom::Start(2)).unwrap();
        let (bytes, _, err) = drain(&mut s, 8);
        assert_eq!(bytes, [2, 3, 4]);
        assert_eq!(err, Some(FAIL));
        // Seeking past the fault is fine; reading there is not.
        s.seek(SeekFrom::Start(7)).unwrap();
        assert_eq!(drain(&mut s, 8), (vec![], vec![], Some(FAIL)));
    }

    #[test]
    fn not_seekable_reports_and_refuses() {
        let mut s = FaultySource::new(data(10)).not_seekable();
        assert!(!s.is_seekable());
        assert_eq!(s.byte_len(), None);
        assert!(s.seek(SeekFrom::Start(0)).is_err());
        assert_eq!(drain(&mut s, 4).0, (0..10).collect::<Vec<u8>>());
    }

    #[test]
    fn short_reads_and_a_fault_combine() {
        let mut s = FaultySource::new(data(10)).max_read(2).fail_at(5, FAIL);
        let (bytes, sizes, err) = drain(&mut s, 8);
        assert_eq!(bytes, [0, 1, 2, 3, 4]);
        assert_eq!(sizes, [2, 2, 1]);
        assert_eq!(err, Some(FAIL));
    }

    #[test]
    fn truncation_before_the_fault_ends_cleanly() {
        let mut s = FaultySource::new(data(10)).truncate_at(3).fail_at(5, FAIL);
        assert_eq!(drain(&mut s, 8), (vec![0, 1, 2], vec![3], None));
        let mut s = FaultySource::new(data(10)).truncate_at(8).fail_at(5, FAIL);
        assert_eq!(drain(&mut s, 8).2, Some(FAIL));
    }

    #[test]
    fn unseekable_short_reads_truncated() {
        let mut s = FaultySource::new(data(10))
            .not_seekable()
            .max_read(4)
            .truncate_at(6);
        assert_eq!(s.byte_len(), None);
        assert_eq!(drain(&mut s, 8), (vec![0, 1, 2, 3, 4, 5], vec![4, 2], None));
    }

    #[test]
    fn faulty_source_is_a_media_source_object() {
        let boxed: Box<dyn MediaSource> = Box::new(FaultySource::new(data(3)).max_read(1));
        let mut boxed = boxed;
        assert_eq!(drain(boxed.as_mut(), 8).1, [1, 1, 1]);
    }

    fn sink() -> Cursor<Vec<u8>> {
        Cursor::new(Vec::new())
    }

    #[test]
    fn sink_passes_through_by_default() {
        let mut s = FaultySink::new(sink());
        s.write_all(b"abc").unwrap();
        MediaSink::seek(&mut s, SeekFrom::Start(0)).unwrap();
        s.write_all(b"X").unwrap();
        s.finish().unwrap();
        assert_eq!(s.into_inner().into_inner(), b"Xbc");
    }

    #[test]
    fn sink_fail_after_writes_the_part_that_fits_then_errors() {
        let mut s = FaultySink::new(sink()).fail_after(5, FAIL);
        s.write_all(b"abc").unwrap();
        assert_eq!(s.write_all(b"defg").unwrap_err().kind(), FAIL);
        assert_eq!(s.write_all(b"h").unwrap_err().kind(), FAIL);
        s.write_all(b"").unwrap();
        assert_eq!(s.into_inner().into_inner(), b"abcde");
    }

    #[test]
    fn sink_exactly_at_the_limit_succeeds() {
        let mut s = FaultySink::new(sink()).fail_after(3, ErrorKind::StorageFull);
        s.write_all(b"abc").unwrap();
        assert_eq!(
            s.write_all(b"d").unwrap_err().kind(),
            ErrorKind::StorageFull
        );
    }

    #[test]
    fn sink_fail_on_finish_only_fails_finish() {
        let mut s = FaultySink::new(sink()).fail_on_finish(ErrorKind::Other);
        s.write_all(b"abc").unwrap();
        assert_eq!(s.finish().unwrap_err().kind(), ErrorKind::Other);
    }

    #[test]
    fn sink_options_combine() {
        let mut s = FaultySink::new(sink())
            .fail_after(2, FAIL)
            .fail_on_finish(ErrorKind::BrokenPipe);
        assert!(s.write_all(b"abc").is_err());
        assert_eq!(s.finish().unwrap_err().kind(), ErrorKind::BrokenPipe);
        assert_eq!(s.into_inner().into_inner(), b"ab");
    }

    #[test]
    fn faulty_sink_is_a_media_sink_object() {
        let mut boxed: Box<dyn MediaSink> = Box::new(FaultySink::new(sink()));
        boxed.write_all(b"ok").unwrap();
        boxed.finish().unwrap();
    }
}
