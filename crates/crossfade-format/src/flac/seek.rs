//! Seeking in native FLAC.
//!
//! Frames are independent, so a seek lands on the frame that holds the target and needs no
//! pre-roll. A SEEKTABLE (RFC 9639 section 8.5), when its points check out, narrows the search;
//! then the audio's bytes are bisected, each probe taking the first frame header past the
//! midpoint that parses (CRC-8), agrees with STREAMINFO and is followed by the next frame where
//! the reader's own boundary logic finds it. So a seek costs O(log n) reads; the last stretch is
//! read frame by frame.

use super::{FlacDemuxer, MAX_HEADER, find_sync};
use crate::wav::{origin, seek_to, source_len};
use crossfade_core::{Demuxer, Error, Packet};
use crossfade_flac::frame::{self, FrameHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SeekPoint {
    pub(super) sample: u64,
    /// From the first byte of the first frame header.
    pub(super) offset: u64,
}

const POINT_LEN: usize = 18;
const PLACEHOLDER: u64 = u64::MAX;
/// Below this many bytes between the brackets, reading frame by frame costs less than probing.
const LINEAR_SPAN: u64 = 64 * 1024;
/// Header-shaped byte patterns one probe checks in full before it gives up on its stretch.
const MAX_CANDIDATES: usize = 64;

/// The table's usable points: placeholders dropped, and any point that does not move forward
/// in both samples and bytes (a table is only a hint, and every point is checked before use).
pub(super) fn parse_seektable(body: &[u8]) -> Vec<SeekPoint> {
    if !body.len().is_multiple_of(POINT_LEN) {
        return Vec::new();
    }
    let mut out: Vec<SeekPoint> = Vec::new();
    for p in body.as_chunks::<POINT_LEN>().0 {
        let sample = u64::from_be_bytes(p[..8].try_into().expect("8 bytes"));
        let offset = u64::from_be_bytes(p[8..16].try_into().expect("8 bytes"));
        if sample == PLACEHOLDER
            || out
                .last()
                .is_some_and(|l| sample <= l.sample || offset <= l.offset)
        {
            continue;
        }
        out.push(SeekPoint { sample, offset });
    }
    out
}

impl FlacDemuxer {
    pub(super) fn seek_frame(&mut self, pts: i64) -> Result<i64, Error> {
        let origin = origin(self.source.as_mut(), self.pos)?;
        if self.first.is_none() {
            // Nothing read yet: the first frame fixes the blocking strategy and where audio
            // starts.
            let Some((h, ..)) = self.first_frame()? else {
                self.done = true;
                return Ok(0);
            };
            self.first = Some((h.variable_block_size, h.block_size));
            self.audio_start = Some(self.base);
        }
        let start = self.audio_start.unwrap_or(0);
        let end = source_len(self.source.as_mut(), origin)?.saturating_sub(self.data_offset);
        let target = u64::try_from(pts).unwrap_or(0);
        if let Some(total) = self.streams[0].frames
            && target >= total
        {
            self.done = true;
            return i64::try_from(total).map_err(|_| Error::Overflow);
        }
        let Some(h) = self.confirm(origin, start)? else {
            return self.walk(origin, start, target);
        };
        let first = self.sample_of(&h);
        if target < first + u64::from(h.block_size) {
            return self.land(origin, start, first);
        }
        let mut lo = (start, first);
        let mut hi = end;
        if let Some(found) = self.narrow_by_table(origin, target, &mut lo, &mut hi)? {
            return self.land(origin, found.0, found.1);
        }
        while hi.saturating_sub(lo.0) > LINEAR_SPAN {
            let mid = lo.0 + (hi - lo.0) / 2;
            match self.probe(origin, mid, hi)? {
                None => hi = mid,
                Some((off, h)) => {
                    let s = self.sample_of(&h);
                    if s > target {
                        hi = mid;
                    } else if s + u64::from(h.block_size) > target {
                        return self.land(origin, off, s);
                    } else {
                        lo = (off, s);
                    }
                }
            }
        }
        self.walk(origin, lo.0, target)
    }

    /// Uses the table's points either side of `target`, each only if a frame with that sample
    /// is where it says. Returns the frame that holds `target` when a point is on it.
    fn narrow_by_table(
        &mut self,
        origin: u64,
        target: u64,
        lo: &mut (u64, u64),
        hi: &mut u64,
    ) -> Result<Option<(u64, u64)>, Error> {
        let start = self.audio_start.unwrap_or(0);
        let i = self.seektable.partition_point(|p| p.sample <= target);
        let at = |p: SeekPoint| start.checked_add(p.offset);
        if let Some(p) = i.checked_sub(1).map(|i| self.seektable[i])
            && let Some(off) = at(p).filter(|&o| o > lo.0 && o < *hi)
            && let Some(h) = self.confirm(origin, off)?
            && self.sample_of(&h) == p.sample
        {
            if p.sample + u64::from(h.block_size) > target {
                return Ok(Some((off, p.sample)));
            }
            *lo = (off, p.sample);
        }
        if let Some(&p) = self.seektable.get(i)
            && let Some(off) = at(p).filter(|&o| o > lo.0 && o < *hi)
            && let Some(h) = self.confirm(origin, off)?
            && self.sample_of(&h) == p.sample
        {
            *hi = off;
        }
        Ok(None)
    }

    /// Reads frame by frame from the frame at `from` to the one that holds `target`, or to the
    /// end.
    fn walk(&mut self, origin: u64, from: u64, target: u64) -> Result<i64, Error> {
        self.reposition(origin, from)?;
        let target = i64::try_from(target).unwrap_or(i64::MAX);
        let mut p = Packet::default();
        let mut end = 0i64;
        loop {
            let at = self.base;
            if !self.read_packet(&mut p)? {
                return Ok(end);
            }
            end = p
                .pts
                .saturating_add(i64::try_from(p.duration).unwrap_or(i64::MAX));
            if end > target {
                return self.land(origin, at, p.pts as u64);
            }
        }
    }

    fn land(&mut self, origin: u64, offset: u64, sample: u64) -> Result<i64, Error> {
        self.reposition(origin, offset)?;
        i64::try_from(sample).map_err(|_| Error::Overflow)
    }

    /// Restarts reading at stream offset `offset`, which must be a frame header.
    fn reposition(&mut self, origin: u64, offset: u64) -> Result<(), Error> {
        let at = self
            .data_offset
            .checked_add(offset)
            .ok_or(Error::Overflow)?;
        seek_to(self.source.as_mut(), origin, &mut self.pos, at)?;
        self.buf.clear();
        self.base = offset;
        self.eof = false;
        self.done = false;
        self.reset_scan();
        Ok(())
    }

    fn sample_of(&self, h: &FrameHeader) -> u64 {
        h.first_sample(self.first.map_or(0, |(_, block)| block))
    }

    /// A header that could belong to this stream: its blocking strategy, its channels, depth and
    /// rate, and a position inside it.
    fn plausible(&self, h: &FrameHeader) -> bool {
        let (channels, bits, rate) = self.format;
        self.first
            .is_some_and(|(variable, _)| variable == h.variable_block_size)
            && h.channels.channels() == channels
            && h.bits_per_sample.is_none_or(|b| b == bits)
            && h.sample_rate.is_none_or(|r| r == rate)
            && self.streams[0]
                .frames
                .is_none_or(|total| self.sample_of(h) < total)
    }

    /// The header of the frame at `offset` if one starts there: it parses, fits the stream, and
    /// the frame's end is found where the next frame's number says it should be (or the input
    /// ends with it).
    fn confirm(&mut self, origin: u64, offset: u64) -> Result<Option<FrameHeader>, Error> {
        self.reposition(origin, offset)?;
        self.ensure(MAX_HEADER)?;
        let Some(h) = frame::parse_header(&self.buf)
            .ok()
            .filter(|h| self.plausible(h))
        else {
            return Ok(None);
        };
        let (end, last) = match self.frame_end(&h) {
            Ok(found) => found,
            Err(e @ Error::Io { .. }) => return Err(e),
            Err(_) => return Ok(None),
        };
        if last {
            return Ok(Some(h));
        }
        self.ensure(end + MAX_HEADER)?;
        let expected = if h.variable_block_size {
            h.number + u64::from(h.block_size)
        } else {
            h.number + 1
        };
        let follows = frame::parse_header(&self.buf[end..])
            .is_ok_and(|n| n.variable_block_size == h.variable_block_size && n.number == expected);
        Ok(follows.then_some(h))
    }

    /// The first frame that starts at or after `from` and before `limit`.
    fn probe(
        &mut self,
        origin: u64,
        from: u64,
        limit: u64,
    ) -> Result<Option<(u64, FrameHeader)>, Error> {
        self.reposition(origin, from)?;
        let mut i = 0;
        let mut tried = 0;
        loop {
            let Some(j) = find_sync(&self.buf, i) else {
                // Keep a last 0xFF: it may start a sync code.
                let keep = self.buf.len().saturating_sub(1);
                if self.base + keep as u64 >= limit {
                    return Ok(None);
                }
                self.consume(keep);
                i = 0;
                if !self.read_more()? {
                    return Ok(None);
                }
                continue;
            };
            let at = self.base + j as u64;
            if at >= limit {
                return Ok(None);
            }
            self.ensure(j + MAX_HEADER)?;
            let fits = frame::parse_header(&self.buf[j..]).is_ok_and(|h| self.plausible(&h));
            if !fits {
                i = j + 1;
                continue;
            }
            if let Some(h) = self.confirm(origin, at)? {
                return Ok(Some((at, h)));
            }
            tried += 1;
            if tried >= MAX_CANDIDATES {
                return Ok(None);
            }
            self.reposition(origin, at + 1)?;
            i = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(sample: u64, offset: u64) -> Vec<u8> {
        let mut p = sample.to_be_bytes().to_vec();
        p.extend(offset.to_be_bytes());
        p.extend(4096u16.to_be_bytes());
        p
    }

    #[test]
    fn placeholders_and_points_that_do_not_move_forward_are_dropped() {
        let body = [
            point(0, 0),
            point(4096, 100),
            point(PLACEHOLDER, 0),
            point(4096, 200),
            point(8192, 100),
            point(9000, 300),
            point(PLACEHOLDER, 0),
        ]
        .concat();
        assert_eq!(
            parse_seektable(&body),
            [
                SeekPoint {
                    sample: 0,
                    offset: 0
                },
                SeekPoint {
                    sample: 4096,
                    offset: 100
                },
                SeekPoint {
                    sample: 9000,
                    offset: 300
                },
            ]
        );
    }

    #[test]
    fn a_table_whose_length_is_not_whole_points_is_ignored() {
        let mut body = point(0, 0);
        body.push(1);
        assert!(parse_seektable(&body).is_empty());
        assert!(parse_seektable(&[]).is_empty());
    }
}
