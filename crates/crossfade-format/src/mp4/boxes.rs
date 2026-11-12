//! ISO base media boxes: headers read from the source at the top level, and children walked in
//! memory below it (the `moov` box is read whole, within the allocation limit).

use crossfade_core::{Error, MediaSource};
use std::io::ErrorKind;

pub(super) type FourCc = [u8; 4];

pub(super) fn malformed(what: impl Into<String>) -> Error {
    Error::Malformed {
        what: format!("mp4: {}", what.into()),
    }
}

/// A four-character code as text, for messages: printable ASCII as it is, anything else as
/// hex.
pub(super) fn name(code: FourCc) -> String {
    if code.iter().all(|b| (0x20..0x7F).contains(b)) {
        code.iter().map(|&b| char::from(b)).collect()
    } else if code[0] == 0xA9 && code[1..].iter().all(u8::is_ascii_graphic) {
        std::iter::once('©')
            .chain(code[1..].iter().map(|&b| char::from(b)))
            .collect()
    } else {
        format!("0x{:08X}", u32::from_be_bytes(code))
    }
}

/// A top-level box as read from the source: its type, where its body starts and how long the
/// body is (`None`: to the end of the input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TopBox {
    pub(super) kind: FourCc,
    pub(super) body_start: u64,
    pub(super) body_len: Option<u64>,
}

/// Fills `buf` unless the source ends first; returns the bytes read.
pub(super) fn fill(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    buf: &mut [u8],
) -> Result<usize, Error> {
    let mut n = 0;
    while n < buf.len() {
        match source.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k.min(buf.len() - n),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    *pos = pos.saturating_add(n as u64);
    Ok(n)
}

pub(super) fn fill_exact(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    buf: &mut [u8],
) -> Result<(), Error> {
    if fill(source, pos, buf)? < buf.len() {
        return Err(Error::UnexpectedEof { offset: *pos });
    }
    Ok(())
}

/// Moves forward to `to`: by seeking when the source can, else by reading.
pub(super) fn skip_to(source: &mut dyn MediaSource, pos: &mut u64, to: u64) -> Result<(), Error> {
    if to < *pos {
        return Err(malformed("a box runs backwards"));
    }
    if source.is_seekable() {
        if to != *pos {
            source.seek(std::io::SeekFrom::Start(to))?;
            *pos = to;
        }
        return Ok(());
    }
    let mut scratch = [0u8; 8192];
    while *pos < to {
        let step = scratch.len().min(usize::try_from(to - *pos).unwrap_or(usize::MAX));
        fill_exact(source, pos, &mut scratch[..step])?;
    }
    Ok(())
}

/// Reads `len` bytes into a vector that grows only as bytes arrive, so a declared length the
/// input cannot back costs nothing. `None` reads to the end of the input, up to `max`.
pub(super) fn read_body(
    source: &mut dyn MediaSource,
    pos: &mut u64,
    len: Option<u64>,
    max: usize,
) -> Result<Vec<u8>, Error> {
    if let Some(len) = len
        && len > max as u64
    {
        return Err(Error::AllocLimitExceeded {
            requested: usize::try_from(len).unwrap_or(usize::MAX),
            max,
        });
    }
    let want = len.map_or(usize::MAX, |l| l as usize);
    let mut out = Vec::new();
    let mut chunk = [0u8; 64 << 10];
    while out.len() < want {
        let step = chunk.len().min(want - out.len());
        let got = fill(source, pos, &mut chunk[..step])?;
        if got == 0 {
            if len.is_none() {
                break;
            }
            return Err(Error::UnexpectedEof { offset: *pos });
        }
        if out.len() + got > max {
            return Err(Error::AllocLimitExceeded {
                requested: out.len() + got,
                max,
            });
        }
        out.extend_from_slice(&chunk[..got]);
    }
    Ok(out)
}

/// The next top-level box header, `None` at a clean end of input.
pub(super) fn next_top(source: &mut dyn MediaSource, pos: &mut u64) -> Result<Option<TopBox>, Error> {
    let start = *pos;
    let mut head = [0u8; 8];
    match fill(source, pos, &mut head)? {
        0 => return Ok(None),
        8 => {}
        _ => return Err(Error::UnexpectedEof { offset: *pos }),
    }
    let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
    let kind = [head[4], head[5], head[6], head[7]];
    let (total, header) = match size {
        0 => (None, 8),
        1 => {
            let mut large = [0u8; 8];
            fill_exact(source, pos, &mut large)?;
            (Some(u64::from_be_bytes(large)), 16)
        }
        n => (Some(u64::from(n)), 8),
    };
    let body_len = match total {
        None => None,
        Some(t) if t < header => {
            return Err(malformed(format!(
                "box '{}' is shorter than its header",
                name(kind)
            )));
        }
        Some(t) => Some(t - header),
    };
    Ok(Some(TopBox {
        kind,
        body_start: start + header,
        body_len,
    }))
}

/// Walks the boxes in a body held in memory.
pub(super) struct Children<'a> {
    rest: &'a [u8],
}

impl<'a> Children<'a> {
    pub(super) fn new(body: &'a [u8]) -> Self {
        Self { rest: body }
    }
}

impl<'a> Iterator for Children<'a> {
    type Item = Result<(FourCc, &'a [u8]), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.rest;
        if rest.len() < 8 {
            // A few bytes of padding after the last box (QuickTime ends some lists with four
            // zero bytes) are not a box.
            self.rest = &[];
            return None;
        }
        let size = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]);
        let kind = [rest[4], rest[5], rest[6], rest[7]];
        let (total, header) = match size {
            0 => (rest.len() as u64, 8),
            1 => match rest.get(8..16) {
                Some(b) => (
                    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
                    16,
                ),
                None => {
                    self.rest = &[];
                    return Some(Err(malformed(format!("box '{}' is cut off", name(kind)))));
                }
            },
            n => (u64::from(n), 8),
        };
        if total < header || total > rest.len() as u64 {
            self.rest = &[];
            return Some(Err(malformed(format!(
                "box '{}' does not fit inside its parent",
                name(kind)
            ))));
        }
        let total = total as usize;
        self.rest = &rest[total..];
        Some(Ok((kind, &rest[header as usize..total])))
    }
}

/// The first child of `kind`, if any; a broken sibling before it is an error.
pub(super) fn find<'a>(body: &'a [u8], kind: &FourCc) -> Result<Option<&'a [u8]>, Error> {
    for child in Children::new(body) {
        let (k, b) = child?;
        if &k == kind {
            return Ok(Some(b));
        }
    }
    Ok(None)
}

/// A cursor over a box body, every read bounds-checked.
pub(super) struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Cursor<'a> {
    /// `what` names the box in errors.
    pub(super) fn new(data: &'a [u8], what: &'static str) -> Self {
        Self { data, pos: 0, what }
    }

    pub(super) fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        let Some(end) = end else {
            return Err(malformed(format!("'{}' box is cut off", self.what)));
        };
        let b = &self.data[self.pos..end];
        self.pos = end;
        Ok(b)
    }

    pub(super) fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.bytes(n).map(drop)
    }

    pub(super) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1)?[0])
    }

    pub(super) fn u16(&mut self) -> Result<u16, Error> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(super) fn u32(&mut self) -> Result<u32, Error> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(super) fn u64(&mut self) -> Result<u64, Error> {
        let b = self.bytes(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// A full box's version and flags.
    pub(super) fn version(&mut self) -> Result<u8, Error> {
        let v = self.u8()?;
        self.skip(3)?;
        Ok(v)
    }

    pub(super) fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    pub(super) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// A table's entry count, checked against the bytes left for its entries.
    pub(super) fn count(&mut self, entry_bytes: usize) -> Result<usize, Error> {
        let n = self.u32()? as usize;
        if n.checked_mul(entry_bytes)
            .is_none_or(|bytes| bytes > self.remaining())
        {
            return Err(malformed(format!(
                "'{}' box has more entries than bytes",
                self.what
            )));
        }
        Ok(n)
    }
}
