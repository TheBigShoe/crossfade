//! Finds MPEG audio frames in a byte stream that may be a pipe.
//!
//! A frame is trusted when it starts exactly where the previous one ended and its header
//! belongs to the same stream. Anywhere else (the first frame, after garbage) a candidate
//! header counts only when another header of the same stream sits where the candidate says
//! its frame ends, so byte patterns inside junk or audio data that merely look like a sync
//! word are passed over.

use crossfade_core::{Error, MediaSource};
use crossfade_mp3::FrameHeader;
use std::io::{ErrorKind, SeekFrom};

const READ_CHUNK: usize = 64 << 10;
/// The longest free-format frame searched for: MPEG-1 Layer III at 640 kbit/s and 32 kHz is
/// 2880 bytes, so this leaves room.
pub(super) const MAX_FREE_FORMAT: usize = 4096;
/// Free-format candidates whose length is searched for before giving up on free format, so
/// junk full of free-format-looking headers costs a bounded amount of scanning.
const MAX_FREE_SEARCHES: usize = 16;

/// One frame: where it starts in the input, how long it is, and its header.
#[derive(Debug, Clone, Copy)]
pub(super) struct Frame {
    pub(super) offset: u64,
    pub(super) len: usize,
    pub(super) header: FrameHeader,
}

/// Why `next` stopped: the stream ended, or a frame of another stream begins there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stop {
    End,
    /// A run of consistent frames with another version, layer or sample rate starts here.
    Changed {
        offset: u64,
        header: FrameHeader,
    },
}

pub(super) struct Framer {
    source: Box<dyn MediaSource>,
    /// Read-ahead: `buf[0]` is input offset `base`, `buf[at..]` is not yet consumed.
    buf: Vec<u8>,
    at: usize,
    base: u64,
    eof: bool,
    /// Where the audio ends (trailing tags excluded), when known.
    end: Option<u64>,
    /// The stream's first frame's header: later frames must agree with it.
    stream: Option<FrameHeader>,
    /// The length of an unpadded free-format frame, once found.
    free_len: Option<usize>,
    free_searches: usize,
    /// The previous frame ended exactly at the read position.
    synced: bool,
    /// How far a search for a frame may scan before giving up, from where it started.
    max_junk: u64,
}

impl Framer {
    /// `base` is the input offset `source` reads from next.
    pub(super) fn new(source: Box<dyn MediaSource>, base: u64, max_junk: u64) -> Self {
        Self {
            source,
            buf: Vec::new(),
            at: 0,
            base,
            eof: false,
            end: None,
            stream: None,
            free_len: None,
            free_searches: 0,
            synced: false,
            max_junk,
        }
    }

    pub(super) fn source(&self) -> &dyn MediaSource {
        &*self.source
    }

    pub(super) fn set_end(&mut self, end: Option<u64>) {
        self.end = end;
    }

    pub(super) fn set_max_junk(&mut self, max: u64) {
        self.max_junk = max;
    }

    pub(super) fn stream(&self) -> Option<FrameHeader> {
        self.stream
    }

    /// Input offset of the next unconsumed byte.
    pub(super) fn position(&self) -> u64 {
        self.base + self.at as u64
    }

    /// The bytes of `frame`, which `next` just returned.
    pub(super) fn bytes(&self, frame: &Frame) -> &[u8] {
        let start = (frame.offset - self.base) as usize;
        &self.buf[start..start + frame.len]
    }

    /// Moves to input offset `offset`, where a frame of the known stream starts (`synced`) or
    /// from where to search for one. Needs a seekable source.
    pub(super) fn reposition(&mut self, offset: u64, synced: bool) -> Result<(), Error> {
        self.source.seek(SeekFrom::Start(offset))?;
        self.buf.clear();
        self.at = 0;
        self.base = offset;
        self.eof = false;
        self.synced = synced;
        Ok(())
    }

    /// Bytes available from the read position, after reading until at least `n` are or the
    /// audio ends.
    fn fill(&mut self, n: usize) -> Result<usize, Error> {
        while self.buf.len() - self.at < n && !self.eof {
            if self.at > 0 && self.at >= self.buf.len() / 2 {
                self.buf.drain(..self.at);
                self.base += self.at as u64;
                self.at = 0;
            }
            let have = self.buf.len();
            let want = READ_CHUNK.max(n - (have - self.at));
            self.buf.resize(have + want, 0);
            let got = loop {
                match self.source.read(&mut self.buf[have..]) {
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    other => break other,
                }
            };
            match got {
                Ok(0) => {
                    self.buf.truncate(have);
                    self.eof = true;
                }
                Ok(k) => self.buf.truncate(have + k.min(want)),
                Err(e) => {
                    self.buf.truncate(have);
                    return Err(e.into());
                }
            }
        }
        let mut avail = self.buf.len() - self.at;
        if let Some(end) = self.end {
            let left = end.saturating_sub(self.position());
            avail = avail.min(usize::try_from(left).unwrap_or(usize::MAX));
        }
        Ok(avail)
    }

    /// The header at `rel` bytes past the read position, if the bytes are there.
    fn header_at(&self, rel: usize, avail: usize) -> Option<FrameHeader> {
        if rel + 4 > avail {
            return None;
        }
        let b = &self.buf[self.at + rel..self.at + rel + 4];
        FrameHeader::parse(&[b[0], b[1], b[2], b[3]]).ok()
    }

    /// The length of the frame `h` heads, if it can be known.
    fn len_of(&self, h: &FrameHeader) -> Option<usize> {
        match h.frame_len() {
            Some(len) => Some(len),
            None => self.free_len.map(|n| h.free_format_len(n)),
        }
    }

    /// The next frame, or why there is none.
    pub(super) fn next(&mut self) -> Result<Result<Frame, Stop>, Error> {
        if self.synced {
            let avail = self.fill(4)?;
            if let Some(h) = self.header_at(0, avail) {
                let same = self.stream.is_none_or(|s| s.same_stream(&h));
                if let Some(len) = self.len_of(&h).filter(|_| same) {
                    let avail = self.fill(len + 4)?;
                    if len > avail {
                        return Ok(Err(Stop::End));
                    }
                    if self.followed_well(&h, len, avail) {
                        return Ok(Ok(self.take(h, len)));
                    }
                }
            }
            self.synced = false;
        }
        self.search()
    }

    /// A frame where the last one ended is taken when what follows it is another frame of
    /// the stream, the end of the audio, or a tag; a frame cut short by garbage is not.
    fn followed_well(&self, h: &FrameHeader, len: usize, avail: usize) -> bool {
        if len + 4 > avail {
            return len == avail || avail - len < 4 && self.eof_reached();
        }
        if self
            .header_at(len, avail)
            .is_some_and(|n| h.same_stream(&n))
        {
            return true;
        }
        let rest = &self.buf[self.at + len..self.at + avail];
        [&b"TAG"[..], b"APETAGEX", b"ID3", b"LYRICSBEGIN"]
            .iter()
            .any(|t| rest.starts_with(t) || t.starts_with(rest))
    }

    fn take(&mut self, header: FrameHeader, len: usize) -> Frame {
        let frame = Frame {
            offset: self.position(),
            len,
            header,
        };
        self.at += len;
        self.synced = true;
        if self.stream.is_none() {
            self.stream = Some(header);
        }
        frame
    }

    /// Scans for a header that another header confirms.
    fn search(&mut self) -> Result<Result<Frame, Stop>, Error> {
        let started = self.position();
        loop {
            // Candidates may start at most `max_junk` bytes past where the search began.
            let room = started.saturating_add(self.max_junk) - self.position();
            // Enough for a candidate, its frame and the confirming header after it.
            let avail = self.fill(2 * MAX_FREE_FORMAT + 8)?;
            if avail < 4 {
                return Ok(Err(Stop::End));
            }
            let span = (avail - 3).min(
                usize::try_from(room)
                    .unwrap_or(usize::MAX)
                    .saturating_add(1),
            );
            let window = &self.buf[self.at..self.at + span];
            let Some(skip) = window.iter().position(|&b| b == 0xFF) else {
                if span as u64 > room {
                    return Err(Error::Malformed {
                        what: format!(
                            "mp3: no MPEG audio frame in the {} bytes from byte {started}",
                            self.max_junk
                        ),
                    });
                }
                self.at += span;
                continue;
            };
            self.at += skip;
            let avail = avail - skip;
            if let Some(found) = self.candidate(avail) {
                return Ok(found);
            }
            self.at += 1;
        }
    }

    /// Judges the header at the read position.
    fn candidate(&mut self, avail: usize) -> Option<Result<Frame, Stop>> {
        let h = self.header_at(0, avail)?;
        let len = match self.len_of(&h) {
            Some(len) => len,
            None => self.find_free_len(&h, avail)?,
        };
        if len < h.data_start() + h.side_info_len() {
            return None;
        }
        if len + 4 > avail {
            // Only a frame that ends exactly where the audio does needs no confirmation.
            let fits = len == avail && self.eof_reached();
            return fits
                .then(|| self.stream.is_none_or(|s| s.same_stream(&h)))
                .filter(|&same| same)
                .map(|_| Ok(self.take(h, len)));
        }
        let next = self.header_at(len, avail)?;
        if !h.same_stream(&next) {
            return None;
        }
        match self.stream {
            Some(s) if !s.same_stream(&h) => Some(Err(Stop::Changed {
                offset: self.position(),
                header: h,
            })),
            _ => Some(Ok(self.take(h, len))),
        }
    }

    fn eof_reached(&self) -> bool {
        self.eof
            || self
                .end
                .is_some_and(|e| self.base + self.buf.len() as u64 >= e)
    }

    /// For a free-format candidate at the read position: the distance to the next header of
    /// the same stream, confirmed by a third where that one says it ends. Remembers the
    /// unpadded length for the rest of the stream.
    fn find_free_len(&mut self, h: &FrameHeader, avail: usize) -> Option<usize> {
        if self.free_searches >= MAX_FREE_SEARCHES {
            return None;
        }
        self.free_searches += 1;
        let min = h.data_start() + h.side_info_len();
        let pad = h.free_format_len(0);
        for k in min.max(pad + 1)..MAX_FREE_FORMAT.min(avail.saturating_sub(4)) {
            if self.buf[self.at + k] != 0xFF {
                continue;
            }
            let Some(next) = self.header_at(k, avail) else {
                continue;
            };
            if !h.same_stream(&next) {
                continue;
            }
            let unpadded = k - pad;
            let after = k + next.free_format_len(unpadded);
            if self
                .header_at(after, avail)
                .is_some_and(|third| h.same_stream(&third))
            {
                self.free_len = Some(unpadded);
                return Some(k);
            }
        }
        None
    }
}
