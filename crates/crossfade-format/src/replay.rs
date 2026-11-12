//! Lets `open_demuxer` identify a container from its first bytes without seeking back, so
//! a pipe works: the bytes it consumed are handed to the chosen demuxer again.

use crossfade_core::{Error, MediaSource};
use std::io::{self, SeekFrom};

pub(crate) struct Replay {
    head: Vec<u8>,
    /// How much of `head` has been handed out; equal to `head.len()` once replaying is over.
    pos: usize,
    /// Where `head` starts in the input: bytes before it went elsewhere (a tag).
    base: u64,
    inner: Box<dyn MediaSource>,
}

impl Replay {
    /// Hands out `head`, which starts at byte `base` of the input, then the rest of `inner`,
    /// whose position is just past `head`.
    pub(crate) fn new(head: Vec<u8>, inner: Box<dyn MediaSource>, base: u64) -> Self {
        Self {
            head,
            pos: 0,
            base,
            inner,
        }
    }

    fn replaying(&self) -> bool {
        self.pos < self.head.len()
    }
}

/// Appends to `head` until it holds `n` bytes or the input ends; reads at most 64 KiB at a
/// time, so a length taken from the input allocates only what the input really has.
pub(crate) fn read_up_to(
    inner: &mut dyn MediaSource,
    head: &mut Vec<u8>,
    n: usize,
) -> Result<(), Error> {
    const CHUNK: usize = 64 << 10;
    while head.len() < n {
        let got = head.len();
        head.resize(got + (n - got).min(CHUNK), 0);
        let read = loop {
            match inner.read(&mut head[got..]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                other => break other,
            }
        };
        match read {
            Ok(k) => {
                head.truncate(got + k);
                if k == 0 {
                    break;
                }
            }
            Err(e) => {
                head.truncate(got);
                return Err(e.into());
            }
        }
    }
    Ok(())
}

impl MediaSource for Replay {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.replaying() {
            let n = buf.len().min(self.head.len() - self.pos);
            buf[..n].copy_from_slice(&self.head[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.inner.read(buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if !self.inner.is_seekable() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "source is not seekable",
            ));
        }
        // The underlying position is already past the head; a relative seek must count from
        // where the reader logically is.
        let pos = match pos {
            SeekFrom::Current(d) if self.replaying() => {
                let here = self.base as i64 + self.pos as i64;
                let target = here.checked_add(d).filter(|t| *t >= 0);
                SeekFrom::Start(target.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "seek before start")
                })? as u64)
            }
            other => other,
        };
        let at = self.inner.seek(pos)?;
        self.pos = self.head.len();
        Ok(at)
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }

    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }
}

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

    /// What sniffing does: up to `n` bytes, and a replay of them in front of the rest.
    fn head_and_replay(
        mut inner: Box<dyn MediaSource>,
        n: usize,
    ) -> Result<(Vec<u8>, Replay), Error> {
        let mut head = Vec::new();
        read_up_to(&mut *inner, &mut head, n)?;
        Ok((head.clone(), Replay::new(head, inner, 0)))
    }

    fn sniffed(src: impl MediaSource + 'static, n: usize) -> Replay {
        head_and_replay(Box::new(src), n).unwrap().1
    }

    #[test]
    fn sniff_returns_the_head_and_replays_it_before_the_rest() {
        let data: Vec<u8> = (0..40).collect();
        let (head, mut r) = head_and_replay(Box::new(Cursor::new(data.clone())), 12).unwrap();
        assert_eq!(head, &data[..12]);
        assert_eq!(drain(&mut r), data);
    }

    #[test]
    fn short_inputs_give_a_short_head_even_through_one_byte_reads() {
        let src = FaultySource::new(Cursor::new(vec![1u8, 2, 3])).max_read(1);
        let (head, mut r) = head_and_replay(Box::new(src), 12).unwrap();
        assert_eq!(head, [1, 2, 3]);
        assert_eq!(drain(&mut r), [1, 2, 3]);
        let (head, mut r) = head_and_replay(Box::new(Cursor::new(Vec::new())), 12).unwrap();
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
    fn a_replay_that_starts_past_a_prefix_counts_from_the_start_of_the_input() {
        let data: Vec<u8> = (0..40).collect();
        let mut inner: Box<dyn MediaSource> = Box::new(Cursor::new(data));
        let mut prefix = [0u8; 14];
        assert_eq!(inner.read(&mut prefix).unwrap(), 14);
        // Bytes 0..10 went elsewhere (a tag); 10..14 are replayed.
        let mut r = Replay::new(prefix[10..].to_vec(), inner, 10);
        let mut two = [0u8; 2];
        assert_eq!(r.read(&mut two).unwrap(), 2);
        assert_eq!(two, [10, 11]);
        assert_eq!(r.seek(SeekFrom::Current(3)).unwrap(), 15);
        assert_eq!(r.read(&mut two).unwrap(), 2);
        assert_eq!(two, [15, 16]);
        let mut r = Replay::new(
            vec![10, 11, 12, 13],
            Box::new(Cursor::new(vec![0u8; 40])),
            10,
        );
        assert!(r.seek(SeekFrom::Current(-11)).is_err());
        assert_eq!(r.seek(SeekFrom::Current(-10)).unwrap(), 0);
        let pipe = FaultySource::new(Cursor::new(vec![14u8, 15])).not_seekable();
        let mut r = Replay::new(vec![12, 13], Box::new(pipe), 12);
        assert_eq!(drain(&mut r), [12, 13, 14, 15]);
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
