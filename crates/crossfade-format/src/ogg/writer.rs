//! Ogg page writer (RFC 3533): packets in, pages out.
//!
//! Page policy: a page closes once it holds 4 KiB of body or 255 lacing values, whichever
//! comes first, and a packet is never split unless it alone needs more than 255 values. 4 KiB
//! is libogg's own threshold: pages stay small enough to seek by and big enough that the 27
//! byte header plus lacing costs well under 1%. The decision to close is taken when the next
//! packet arrives, so the last page is always the one that carries end-of-stream.

use super::crc::crc32;

pub(super) const CONTINUED: u8 = 0x01;
pub(super) const BOS: u8 = 0x02;
pub(super) const EOS: u8 = 0x04;

const MAX_SEGMENTS: usize = 255;
const PAGE_TARGET: usize = 4096;
const HEADER_LEN: usize = 27;

/// The lacing values of a packet of `len` bytes: 255s, then one below 255 (0 after an exact
/// multiple of 255).
fn lacing(len: usize) -> impl Iterator<Item = u8> {
    (0..len / 255).map(|_| 255).chain([(len % 255) as u8])
}

/// One serialised page, CRC included.
pub(super) fn page_bytes(
    flags: u8,
    granule: i64,
    serial: u32,
    sequence: u32,
    lacing: &[u8],
    body: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + lacing.len() + body.len());
    out.extend(b"OggS");
    out.push(0);
    out.push(flags);
    out.extend(granule.to_le_bytes());
    out.extend(serial.to_le_bytes());
    out.extend(sequence.to_le_bytes());
    out.extend([0; 4]);
    out.push(lacing.len() as u8);
    out.extend(lacing);
    out.extend(body);
    let crc = crc32(&out);
    out[22..26].copy_from_slice(&crc.to_le_bytes());
    out
}

pub(super) struct PageWriter {
    serial: u32,
    sequence: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    /// Granule of the last packet that ended on the open page, -1 when none has.
    granule: i64,
    /// The last value closed so far; an end-of-stream page with no packet repeats it.
    last_granule: i64,
    /// The previous page ended inside a packet.
    continued: bool,
    out: Vec<u8>,
}

impl PageWriter {
    pub(super) fn new(serial: u32) -> Self {
        Self {
            serial,
            sequence: 0,
            lacing: Vec::new(),
            body: Vec::new(),
            granule: -1,
            last_granule: 0,
            continued: false,
            out: Vec::new(),
        }
    }

    /// Adds one packet; `granule` is the sample count completed once it has ended.
    pub(super) fn packet(&mut self, data: &[u8], granule: u64) {
        let needed = data.len() / 255 + 1;
        let full = self.body.len() >= PAGE_TARGET || self.lacing.len() + needed > MAX_SEGMENTS;
        if full && !self.lacing.is_empty() {
            self.emit(0);
        }
        let granule = i64::try_from(granule).unwrap_or(i64::MAX);
        let mut at = 0;
        let mut values = lacing(data.len()).peekable();
        while let Some(l) = values.next() {
            let end = at + usize::from(l);
            self.lacing.push(l);
            self.body.extend(&data[at..end]);
            at = end;
            if values.peek().is_some() && self.lacing.len() == MAX_SEGMENTS {
                self.emit(0);
            }
        }
        self.granule = granule;
    }

    /// Closes the open page, if any: a header packet ends on a page boundary.
    pub(super) fn end_page(&mut self) {
        if !self.lacing.is_empty() {
            self.emit(0);
        }
    }

    /// Closes the stream with an end-of-stream page (empty if nothing is open).
    pub(super) fn finish(&mut self) {
        self.emit(EOS);
    }

    /// The pages closed since the last call.
    pub(super) fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    fn emit(&mut self, extra: u8) {
        let mut flags = extra;
        if self.continued {
            flags |= CONTINUED;
        }
        if self.sequence == 0 {
            flags |= BOS;
        }
        // A page that ends no packet says so with -1.
        let granule = if self.lacing.is_empty() {
            self.last_granule
        } else if self.lacing.iter().any(|&l| l < 255) {
            self.granule
        } else {
            -1
        };
        self.out.extend(page_bytes(
            flags,
            granule,
            self.serial,
            self.sequence,
            &self.lacing,
            &self.body,
        ));
        self.sequence += 1;
        self.continued = self.lacing.last() == Some(&255);
        if granule != -1 {
            self.last_granule = granule;
        }
        self.lacing.clear();
        self.body.clear();
        self.granule = -1;
    }
}

#[cfg(test)]
mod tests;
