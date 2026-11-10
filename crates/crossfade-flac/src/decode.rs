//! The FLAC decoder (RFC 9639): one whole frame in, one block of raw samples out, with the
//! stream's MD5 and sample total checked once the last frame is in.
//!
//! The decoder knows nothing of containers: whatever splits the stream into frames (a FLAC
//! or Ogg reader) hands each frame over as a slice.

use crate::crc::crc16;
use crate::error::{Error, Result};
use crate::frame::{self, ChannelAssignment, FrameHeader, HeaderError};
use crate::md5::Md5;
use crate::streaminfo::StreamInfoBlock;
use crate::subframe::{Bits, read_subframe};

fn malformed(what: &str) -> Error {
    Error::Malformed(format!("flac: {what}"))
}

/// What [`Decoder::finish`] could and could not check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    /// The hash of the decoded samples equals the one in STREAMINFO.
    Md5Matched,
    /// STREAMINFO carries no hash (all zero); only the sample total, if it has one, was checked.
    NoMd5,
    /// A frame failed earlier, so the total and the hash say nothing about the stream.
    Skipped,
}

/// Decodes the frames of one stream, one after another.
pub struct Decoder {
    info: StreamInfoBlock,
    /// The stream is too deep for `i32` subframes (see `work`).
    wide: bool,
    /// Work buffers for 32-bit streams only, whose side channel needs 33 bits; every other
    /// stream decodes straight into `planes`.
    work: Vec<Vec<i64>>,
    /// The finished block, one buffer per channel, reused from frame to frame.
    planes: Vec<Vec<i32>>,
    md5: Md5,
    md5_bytes: Vec<u8>,
    /// Samples per channel decoded before the current block.
    first: u64,
    decoded: u64,
    block: usize,
    /// A frame failed: the totals and the hash no longer say anything about the stream.
    failed: bool,
    finished: bool,
}

impl Decoder {
    /// `info` must describe 1 to 8 channels of 4 to 32 bits at a non-zero rate, as
    /// [`StreamInfoBlock::parse`] guarantees.
    pub fn new(info: &StreamInfoBlock) -> Result<Self> {
        if !(1..=8).contains(&info.channels) {
            return Err(malformed("channel count outside 1 to 8"));
        }
        if !(4..=32).contains(&info.bits_per_sample) {
            return Err(malformed("bit depth outside 4 to 32"));
        }
        if info.sample_rate == 0 {
            return Err(malformed("sample rate is zero"));
        }
        let channels = usize::from(info.channels);
        Ok(Self {
            info: *info,
            wide: info.bits_per_sample > 31,
            work: vec![Vec::new(); channels],
            planes: vec![Vec::new(); channels],
            md5: Md5::new(),
            md5_bytes: Vec::new(),
            first: 0,
            decoded: 0,
            block: 0,
            failed: false,
            finished: false,
        })
    }

    /// Decodes one whole frame (as split by a container reader), checking its CRC-16. Zero
    /// bytes after the frame are tolerated: a reader cannot tell them from the frame's end.
    ///
    /// The block borrows the decoder, so it is gone before the next frame is decoded. After an
    /// error the decoder still takes frames, but [`finish`](Self::finish) reports `Skipped`.
    pub fn decode_frame(&mut self, frame: &[u8]) -> Result<Block<'_>> {
        if let Err(e) = self.decode(frame) {
            self.failed = true;
            return Err(e);
        }
        Ok(Block {
            planes: &self.planes,
            len: self.block,
            bits: self.info.bits_per_sample,
            first: self.first,
        })
    }

    /// Call after the last frame: checks the sample total and the MD5 against STREAMINFO, when
    /// it has them. A mismatch, or a stream shorter or longer than announced, is an error.
    pub fn finish(&mut self) -> Result<Verification> {
        if self.failed || std::mem::replace(&mut self.finished, true) {
            return Ok(Verification::Skipped);
        }
        let total = self.info.total_samples;
        if total != 0 && total != self.decoded {
            return Err(malformed(&format!(
                "stream ended after {} of {total} samples",
                self.decoded
            )));
        }
        if self.info.md5 == [0; 16] {
            return Ok(Verification::NoMd5);
        }
        if std::mem::take(&mut self.md5).finalize() != self.info.md5 {
            return Err(malformed(
                "decoded audio does not match the MD5 in the stream header",
            ));
        }
        Ok(Verification::Md5Matched)
    }

    fn check_parameters(&self, h: &FrameHeader) -> Result<()> {
        let unsupported = |what: &str, was: u32, now: u32| {
            Error::Unsupported(format!(
                "flac: {what} changed from {was} to {now} mid-stream"
            ))
        };
        let channels = h.channels.channels();
        if channels != self.info.channels {
            return Err(unsupported(
                "channel count",
                u32::from(self.info.channels),
                u32::from(channels),
            ));
        }
        let bits = h.bits_per_sample.unwrap_or(self.info.bits_per_sample);
        if bits != self.info.bits_per_sample {
            return Err(unsupported(
                "bit depth",
                u32::from(self.info.bits_per_sample),
                u32::from(bits),
            ));
        }
        let rate = h.sample_rate.unwrap_or(self.info.sample_rate);
        if rate != self.info.sample_rate {
            return Err(unsupported("sample rate", self.info.sample_rate, rate));
        }
        Ok(())
    }

    fn decode(&mut self, data: &[u8]) -> Result<()> {
        let h = frame::parse_header(data).map_err(|e| match e {
            HeaderError::NeedMoreData => malformed("frame header cut short"),
            HeaderError::Invalid(why) => malformed(why),
        })?;
        self.check_parameters(&h)?;
        if data.len() < h.header_len + 2 || crc16(data) != 0 {
            return Err(malformed("frame CRC-16 mismatch"));
        }
        let block = h.block_size as usize;
        let bps = u32::from(self.info.bits_per_sample);
        let mut r = Bits::new(&data[h.header_len..data.len() - 2]);
        // The side channel of a stereo pair has one more bit.
        let side = match h.channels {
            ChannelAssignment::LeftSide | ChannelAssignment::MidSide => Some(1),
            ChannelAssignment::SideRight => Some(0),
            ChannelAssignment::Independent(_) => None,
        };
        let side_channel = |c: usize| bps + u32::from(side == Some(c));
        if self.wide {
            for (c, out) in self.work.iter_mut().enumerate() {
                read_subframe(&mut r, out, block, side_channel(c))?;
            }
        } else {
            for (c, out) in self.planes.iter_mut().enumerate() {
                read_subframe(&mut r, out, block, side_channel(c))?;
            }
        }
        r.align();
        // The CRC-16 follows the subframes. Zero bytes after it are padding the container
        // reader could not tell from the frame (the CRC-16 stays zero over zeros).
        let end = data.len() - 2 - (r.bits_left() / 8) as usize;
        if end + 2 < data.len() {
            if crc16(&data[..end + 2]) != 0 {
                return Err(malformed("frame CRC-16 mismatch"));
            }
            if data[end + 2..].iter().any(|&b| b != 0) {
                return Err(malformed(
                    "unused bytes between the subframes and the CRC-16",
                ));
            }
        }
        if self.wide {
            self.decorrelate(h.channels);
            self.narrow(block);
        } else {
            decorrelate_i32(&mut self.planes, h.channels);
        }
        self.hash(block);
        self.first = self.decoded;
        self.decoded += block as u64;
        self.block = block;
        Ok(())
    }

    fn decorrelate(&mut self, assignment: ChannelAssignment) {
        if matches!(assignment, ChannelAssignment::Independent(_)) {
            return;
        }
        // The other assignments are stereo pairs, so two buffers exist.
        let (first, rest) = self.work.split_at_mut(1);
        let (a, b) = (&mut first[0], &mut rest[0]);
        match assignment {
            ChannelAssignment::Independent(_) => {}
            ChannelAssignment::LeftSide => {
                for (l, s) in a.iter().zip(b.iter_mut()) {
                    *s = l.wrapping_sub(*s);
                }
            }
            ChannelAssignment::SideRight => {
                for (s, r) in a.iter_mut().zip(b.iter()) {
                    *s = s.wrapping_add(*r);
                }
            }
            ChannelAssignment::MidSide => {
                for (m, s) in a.iter_mut().zip(b.iter_mut()) {
                    // The mid channel dropped the low bit of L + R; the side's parity restores it.
                    let sum = (m.wrapping_shl(1)) | (*s & 1);
                    *m = sum.wrapping_add(*s) >> 1;
                    *s = sum.wrapping_sub(*s) >> 1;
                }
            }
        }
    }

    /// Truncates the decorrelated channels to `i32`: exact for a valid stream, and the same
    /// low bits as before for a corrupt one that got past the CRC.
    fn narrow(&mut self, block: usize) {
        for (plane, work) in self.planes.iter_mut().zip(&self.work) {
            plane.clear();
            plane.reserve(block);
            plane.extend(work.iter().map(|&v| v as i32));
        }
    }

    /// Hashes the raw samples as the format defines it: interleaved, little-endian, in the
    /// smallest whole number of bytes the depth fits.
    fn hash(&mut self, block: usize) {
        let nch = self.planes.len();
        let width = usize::from(self.info.bits_per_sample).div_ceil(8);
        self.md5_bytes.clear();
        self.md5_bytes.resize(block * nch * width, 0);
        for (c, plane) in self.planes.iter().enumerate() {
            let slots = self.md5_bytes.chunks_exact_mut(width).skip(c).step_by(nch);
            for (slot, &v) in slots.zip(plane) {
                slot.copy_from_slice(&v.to_le_bytes()[..width]);
            }
        }
        self.md5.update(&self.md5_bytes);
    }
}

/// Turns the channels of a stereo pair back into left and right, in place.
fn decorrelate_i32(planes: &mut [Vec<i32>], assignment: ChannelAssignment) {
    if matches!(assignment, ChannelAssignment::Independent(_)) {
        return;
    }
    // The other assignments are stereo pairs, so two buffers exist.
    let (first, rest) = planes.split_at_mut(1);
    let (a, b) = (&mut first[0], &mut rest[0]);
    match assignment {
        ChannelAssignment::Independent(_) => {}
        ChannelAssignment::LeftSide => {
            for (l, s) in a.iter().zip(b.iter_mut()) {
                *s = l.wrapping_sub(*s);
            }
        }
        ChannelAssignment::SideRight => {
            for (s, r) in a.iter_mut().zip(b.iter()) {
                *s = s.wrapping_add(*r);
            }
        }
        ChannelAssignment::MidSide => {
            for (m, s) in a.iter_mut().zip(b.iter_mut()) {
                // The mid channel dropped the low bit of L + R; the side's parity restores it.
                let sum = m.wrapping_shl(1) | (*s & 1);
                *m = sum.wrapping_add(*s) >> 1;
                *s = sum.wrapping_sub(*s) >> 1;
            }
        }
    }
}

/// One decoded frame, borrowed from the decoder: raw sign-extended samples per channel.
#[derive(Debug, Clone, Copy)]
pub struct Block<'a> {
    planes: &'a [Vec<i32>],
    len: usize,
    bits: u8,
    first: u64,
}

/// Writes the planes interleaved into `out` through `f`.
fn interleave<T: Copy + Default>(
    planes: &[Vec<i32>],
    len: usize,
    out: &mut Vec<T>,
    f: impl Fn(i32) -> T,
) {
    let nch = planes.len();
    out.clear();
    out.resize(len * nch, T::default());
    for (c, plane) in planes.iter().enumerate() {
        for (dst, &v) in out[c..].iter_mut().step_by(nch).zip(plane) {
            *dst = f(v);
        }
    }
}

impl Block<'_> {
    pub fn channels(&self) -> usize {
        self.planes.len()
    }

    /// Samples per channel.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn bits_per_sample(&self) -> u8 {
        self.bits
    }

    /// Index, per channel, of this block's first sample in the stream: the samples decoded
    /// before it.
    pub fn first_sample(&self) -> u64 {
        self.first
    }

    /// The raw sample values of one channel, sign-extended from `bits_per_sample`, not
    /// left-justified.
    ///
    /// # Panics
    ///
    /// If `index` is not below [`channels`](Self::channels).
    pub fn channel(&self, index: usize) -> &[i32] {
        &self.planes[index]
    }

    /// Interleaves into `out`, replacing its contents, each sample shifted left by `shift`
    /// (below 32) to fill a 32-bit container.
    pub fn interleave_i32(&self, shift: u32, out: &mut Vec<i32>) {
        interleave(self.planes, self.len, out, |v| v << shift);
    }

    /// As [`interleave_i32`](Self::interleave_i32) into 16-bit samples; `shift` is
    /// `16 - bits_per_sample` for a stream that fits.
    pub fn interleave_i16(&self, shift: u32, out: &mut Vec<i16>) {
        interleave(self.planes, self.len, out, |v| (v << shift) as i16);
    }

    /// As [`interleave_i32`](Self::interleave_i32) into unsigned 8-bit samples: the shifted
    /// value plus 128, so 8-bit audio keeps the usual offset binary form.
    pub fn interleave_u8(&self, shift: u32, out: &mut Vec<u8>) {
        interleave(self.planes, self.len, out, |v| {
            (v << shift).wrapping_add(128) as u8
        });
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
