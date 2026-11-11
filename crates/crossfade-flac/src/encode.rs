//! A FLAC encoder (RFC 9639): interleaved samples in, one frame per block out.
//!
//! Fixed block size, 4 to 32 bits per sample, 1 to 8 channels, any rate FLAC can name. Every
//! frame decodes bit-exactly with this crate's [`Decoder`](crate::Decoder), and the final
//! STREAMINFO carries the MD5 of the audio, so `Decoder::finish` returns `Md5Matched`.
//!
//! ```
//! use crossfade_flac::{Decoder, Encoder, EncoderConfig, Verification};
//!
//! let mut encoder = Encoder::new(EncoderConfig::new(44_100, 2, 16))?;
//! let samples: Vec<i32> = (0..2 * 5000).map(|i| (i % 200) * 50 - 5000).collect();
//! let mut frames = Vec::new();
//! let mut ends = Vec::new(); // where each frame ends in `frames`
//! // One call per block; the last may be short. (`write` and `flush` take any chunk size.)
//! for block in samples.chunks(encoder.block_size() * 2) {
//!     encoder.encode_block(block, &mut frames)?;
//!     ends.push(frames.len());
//! }
//! let info = encoder.finish(); // goes in the STREAMINFO block of the file
//!
//! let mut decoder = Decoder::new(&info)?;
//! let mut start = 0;
//! for end in ends {
//!     decoder.decode_frame(&frames[start..end])?;
//!     start = end;
//! }
//! assert_eq!(decoder.finish()?, Verification::Md5Matched);
//! # Ok::<(), crossfade_flac::Error>(())
//! ```
//!
//! # Levels
//!
//! `level` runs from 0 (fastest) to 8 (smallest); each row adds to the ones above it. The
//! sizes track the reference encoder's presets at the same level (see the crate README).
//!
//! | level | block | stereo | fixed orders coded | LPC max order | LPC windows (orders coded) | max Rice partition order |
//! |---|---|---|---|---|---|---|
//! | 0 | 1152 | independent | all 5 | none | none | 3 |
//! | 1 | 1152 | adaptive | all 5 | none | none | 3 |
//! | 2 | 1152 | exhaustive | all 5 | none | none | 3 |
//! | 3 | 4096 | adaptive | best 2 | 6 | full (1) | 4 |
//! | 4 | 4096 | adaptive | best 2 | 8 | full (1) | 4 |
//! | 5 | 4096 | exhaustive | best 2 | 8 | full (2) | 5 |
//! | 6 | 4096 | exhaustive | best 2 | 8 | full (3) | 6 |
//! | 7 | 4096 | exhaustive | best 2 | 12 | full (4) | 6 |
//! | 8 | 4096 | exhaustive | best 2 | 12 | full (3), first and last two thirds (1 each) | 6 |
//!
//! Every level also does the following, and every choice between candidates is made on exact
//! bit counts, never on an estimate:
//!
//! - **Constant and verbatim subframes**, and **wasted bits**: low zero bits common to every
//!   sample of a channel are shifted out and signalled.
//! - **Fixed predictors** of order 0 to 4. Levels 0 to 2 code all five and keep the cheapest.
//!   From level 3 a one-pass estimate (the sum of absolute residuals) picks the two best
//!   orders to code, because linear prediction usually wins anyway.
//! - **Linear prediction** from level 3: a window (Tukey-like, tapered with a smoothstep
//!   polynomial), autocorrelation, one Levinson-Durbin run for every order up to the maximum,
//!   coefficients quantised to 12 bits with the largest shift that fits. The best few orders
//!   by the reference encoder's size estimate (the count in the table) are coded for real; the
//!   cheapest wins.
//! - **Rice coding** with the partition order searched from 0 up to the level's maximum (and
//!   as far as the block size and predictor order allow). Each partition's parameter is the
//!   exact optimum; 5-bit parameters are used when a partition wants one above 14 and they
//!   pay for themselves, and a partition is stored raw (escaped) when that is cheaper.
//! - **Stereo**: level 0 codes channels independently. *Adaptive* levels rank left/side,
//!   side/right and mid/side by a cheap estimate and code only the winning pair;
//!   *exhaustive* levels code left, right, mid and side fully and keep the cheapest pair.
//!   Either way the pair is compared with plain coding by exact bits before it is emitted.
//!
//! # Guarantees
//!
//! - **Never worse than raw.** A frame is never larger than one made of independent verbatim
//!   (or constant) subframes: a verbatim candidate is always in the race, and a decorrelated
//!   pair that loses to it by exact bits is replaced.
//! - **The format's limits hold.** A residual must fit 32-bit two's complement and not be the
//!   most negative value; a predictor or stereo mode that would break that is passed over, down
//!   to verbatim, which always works. The side channel of a 32-bit stream is 33 bits wide and
//!   is handled in `i64`.
//! - **Deterministic.** No threads. Floating point is used only for the window,
//!   autocorrelation, Levinson-Durbin and quantisation, through operations IEEE 754 defines
//!   exactly (`+ - * /`, comparisons, conversions): no `sin`, `cos`, `exp`, `ln`, `powf` or
//!   fused multiply-add, whose results differ between platforms' maths libraries. The same
//!   input and configuration give the same bytes everywhere.
//! - **No panics, bounded memory.** One block of working memory, allocated in
//!   [`Encoder::new`] and reused; steady state allocates nothing but what the caller's `out`
//!   needs. Arithmetic on sample data is either checked against the limits or wraps where the
//!   result is known to fit.

mod analyze;
mod bitsink;
mod fixed;
mod lpc;
mod rice;
mod write;

use crate::error::{Error, Result};
use crate::frame::{self, ChannelAssignment, FrameHeader};
use crate::md5::Md5;
use crate::streaminfo::StreamInfoBlock;
use analyze::{Analyzer, Level, Plan, Stereo, is_constant};
use bitsink::BitSink;

/// What to encode and how hard to try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    /// 1 to 1_048_575.
    pub sample_rate: u32,
    /// 1 to 8.
    pub channels: u8,
    /// 4 to 32.
    pub bits_per_sample: u8,
    /// 0 (fastest) to 8 (smallest); 5 is the default.
    pub level: u8,
}

impl EncoderConfig {
    pub const DEFAULT_LEVEL: u8 = 5;

    pub fn new(sample_rate: u32, channels: u8, bits_per_sample: u8) -> Self {
        Self {
            sample_rate,
            channels,
            bits_per_sample,
            level: Self::DEFAULT_LEVEL,
        }
    }
}

fn malformed(what: &str) -> Error {
    Error::Malformed(format!("flac encoder: {what}"))
}

fn unsupported(what: &str) -> Error {
    Error::Unsupported(format!("flac encoder: {what}"))
}

/// Fixed-block-size FLAC encoder; see the module documentation.
pub struct Encoder {
    config: EncoderConfig,
    block: usize,
    frame_number: u64,
    total: u64,
    min_frame: u32,
    max_frame: u32,
    short_seen: bool,
    done: Option<StreamInfoBlock>,
    md5: Md5,
    md5_bytes: Vec<u8>,
    /// Samples taken by `write` that do not fill a block yet.
    pending: Vec<i32>,
    /// One block, deinterleaved.
    planes: Vec<Vec<i32>>,
    /// Mid and side of a stereo block; the side of a 32-bit stream needs 33 bits.
    mid: Vec<i32>,
    side32: Vec<i32>,
    side64: Vec<i64>,
    /// One plan per channel; a stereo stream has four: left, right, mid, side.
    slots: Vec<Plan>,
    level: Level,
    analyzer: Analyzer,
    #[cfg(test)]
    force: Option<ChannelAssignment>,
}

impl Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self> {
        if !(1..=1_048_575).contains(&config.sample_rate) {
            return Err(unsupported("sample rate must be 1 to 1048575 Hz"));
        }
        if !(1..=8).contains(&config.channels) {
            return Err(unsupported("channel count must be 1 to 8"));
        }
        if !(4..=32).contains(&config.bits_per_sample) {
            return Err(unsupported("bit depth must be 4 to 32"));
        }
        if config.level > 8 {
            return Err(unsupported("level must be 0 to 8"));
        }
        let level = Level::new(config.level);
        let channels = usize::from(config.channels);
        Ok(Self {
            config,
            block: level.block,
            frame_number: 0,
            total: 0,
            min_frame: u32::MAX,
            max_frame: 0,
            short_seen: false,
            done: None,
            md5: Md5::new(),
            md5_bytes: Vec::with_capacity(level.block * channels * 4),
            pending: Vec::with_capacity(level.block * channels),
            planes: vec![vec![0; level.block]; channels],
            mid: Vec::with_capacity(if channels == 2 { level.block } else { 0 }),
            side32: Vec::with_capacity(if channels == 2 { level.block } else { 0 }),
            side64: Vec::with_capacity(if channels == 2 && config.bits_per_sample == 32 {
                level.block
            } else {
                0
            }),
            slots: (0..if channels == 2 { 4 } else { channels })
                .map(|_| Plan::new(level.block))
                .collect(),
            level,
            analyzer: Analyzer::new(level, channels == 2 && config.bits_per_sample == 32),
            #[cfg(test)]
            force: None,
        })
    }

    /// Samples per channel in every frame but the last.
    pub fn block_size(&self) -> usize {
        self.block
    }

    /// Encodes one block and appends exactly one FLAC frame to `out`. `interleaved` holds
    /// raw sign-extended samples, `block_size() * channels` of them, or fewer (a whole
    /// number of sample frames, at least one) for the final block; after a short block no
    /// more may follow. A sample outside the configured depth is `Error::Malformed`.
    pub fn encode_block(&mut self, interleaved: &[i32], out: &mut Vec<u8>) -> Result<()> {
        if self.done.is_some() {
            return Err(malformed("the stream is finished"));
        }
        if self.short_seen {
            return Err(malformed("a short block must be the last"));
        }
        let channels = usize::from(self.config.channels);
        if interleaved.is_empty() || !interleaved.len().is_multiple_of(channels) {
            return Err(malformed(
                "a block is a whole number of sample frames, at least one",
            ));
        }
        let n = interleaved.len() / channels;
        if n > self.block {
            return Err(malformed("block is longer than block_size()"));
        }
        if !self.pending.is_empty() {
            return Err(malformed("samples from write() are pending; flush() first"));
        }
        self.check_range(interleaved)?;
        if self.frame_number >= 1 << 31 || self.total + n as u64 >= 1 << 36 {
            return Err(unsupported("stream is longer than FLAC can number"));
        }

        for plane in &mut self.planes {
            plane.resize(n, 0);
        }
        match self.planes.as_mut_slice() {
            [mono] => mono.copy_from_slice(interleaved),
            [left, right] => {
                for ((l, r), f) in left
                    .iter_mut()
                    .zip(right)
                    .zip(interleaved.as_chunks::<2>().0)
                {
                    (*l, *r) = (f[0], f[1]);
                }
            }
            planes => {
                for (i, frame) in interleaved.chunks_exact(channels).enumerate() {
                    for (plane, &v) in planes.iter_mut().zip(frame) {
                        plane[i] = v;
                    }
                }
            }
        }
        self.hash(interleaved);

        let start = out.len();
        self.write_frame(n, out);
        let len = (out.len() - start) as u32;
        self.min_frame = self.min_frame.min(len);
        self.max_frame = self.max_frame.max(len);
        self.frame_number += 1;
        self.total += n as u64;
        self.short_seen = n < self.block;
        Ok(())
    }

    fn check_range(&self, samples: &[i32]) -> Result<()> {
        let bps = u32::from(self.config.bits_per_sample);
        if bps < 32 {
            let (lo, hi) = (-(1i32 << (bps - 1)), (1i32 << (bps - 1)) - 1);
            if samples.iter().any(|&s| s < lo || s > hi) {
                return Err(malformed("sample outside the configured bit depth"));
            }
        }
        Ok(())
    }

    /// Takes samples in chunks of any size (whole sample frames) and appends a frame to `out`
    /// each time a block fills. Call [`flush`](Self::flush) at the end for the remainder. Do
    /// not mix with [`encode_block`](Self::encode_block) while samples are pending.
    pub fn write(&mut self, interleaved: &[i32], out: &mut Vec<u8>) -> Result<()> {
        self.write_inner(interleaved, out, None)
    }

    /// As [`write`](Self::write), and pushes `out.len()` onto `ends` after each frame it
    /// appends, so a caller can cut `out` into frames (one packet per frame) without parsing.
    pub fn write_ends(
        &mut self,
        interleaved: &[i32],
        out: &mut Vec<u8>,
        ends: &mut Vec<usize>,
    ) -> Result<()> {
        self.write_inner(interleaved, out, Some(ends))
    }

    fn write_inner(
        &mut self,
        mut interleaved: &[i32],
        out: &mut Vec<u8>,
        mut ends: Option<&mut Vec<usize>>,
    ) -> Result<()> {
        if self.done.is_some() {
            return Err(malformed("the stream is finished"));
        }
        if self.short_seen {
            return Err(malformed("a short block must be the last"));
        }
        let channels = usize::from(self.config.channels);
        if !interleaved.len().is_multiple_of(channels) {
            return Err(malformed("write() takes whole sample frames"));
        }
        self.check_range(interleaved)?;
        let per = self.block * channels;
        if !self.pending.is_empty() {
            let take = (per - self.pending.len()).min(interleaved.len());
            self.pending.extend_from_slice(&interleaved[..take]);
            interleaved = &interleaved[take..];
            if self.pending.len() == per {
                let full = std::mem::take(&mut self.pending);
                let result = self.encode_block(&full, out);
                self.pending = full;
                self.pending.clear();
                result?;
                if let Some(e) = ends.as_deref_mut() {
                    e.push(out.len());
                }
            }
        }
        while interleaved.len() >= per {
            let (block, rest) = interleaved.split_at(per);
            self.encode_block(block, out)?;
            if let Some(e) = ends.as_deref_mut() {
                e.push(out.len());
            }
            interleaved = rest;
        }
        self.pending.extend_from_slice(interleaved);
        Ok(())
    }

    /// Encodes the samples [`write`](Self::write) still holds as the final (short) block. Does
    /// nothing when none are pending. Call it before [`finish`](Self::finish): samples left
    /// pending then are not part of the stream.
    pub fn flush(&mut self, out: &mut Vec<u8>) -> Result<()> {
        self.flush_inner(out, None)
    }

    /// As [`flush`](Self::flush), reporting the frame's end like [`write_ends`](Self::write_ends).
    pub fn flush_ends(&mut self, out: &mut Vec<u8>, ends: &mut Vec<usize>) -> Result<()> {
        self.flush_inner(out, Some(ends))
    }

    fn flush_inner(&mut self, out: &mut Vec<u8>, ends: Option<&mut Vec<usize>>) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let rest = std::mem::take(&mut self.pending);
        let result = self.encode_block(&rest, out);
        self.pending = rest;
        self.pending.clear();
        result?;
        if let Some(e) = ends {
            e.push(out.len());
        }
        Ok(())
    }

    /// The MD5 is over the samples as little-endian integers in the smallest whole number
    /// of bytes the depth fits.
    fn hash(&mut self, interleaved: &[i32]) {
        fn pack<const W: usize>(out: &mut Vec<u8>, samples: &[i32]) {
            out.clear();
            out.resize(samples.len() * W, 0);
            for (dst, &s) in out.as_chunks_mut::<W>().0.iter_mut().zip(samples) {
                dst.copy_from_slice(&s.to_le_bytes()[..W]);
            }
        }
        match self.config.bits_per_sample.div_ceil(8) {
            1 => pack::<1>(&mut self.md5_bytes, interleaved),
            2 => pack::<2>(&mut self.md5_bytes, interleaved),
            3 => pack::<3>(&mut self.md5_bytes, interleaved),
            _ => pack::<4>(&mut self.md5_bytes, interleaved),
        }
        self.md5.update(&self.md5_bytes);
    }

    /// Plans one channel of the block into `slot`: a plane, or for a stereo stream slot 2
    /// is the mid and slot 3 the side channel.
    fn analyze_slot(&mut self, slot: usize, n: usize, bps: u32) {
        let Self {
            analyzer,
            slots,
            planes,
            mid,
            side32,
            side64,
            ..
        } = self;
        let plan = &mut slots[slot];
        if slot < planes.len() {
            analyzer.analyze(&planes[slot][..n], bps, plan);
        } else if slot == 2 {
            analyzer.analyze(&mid[..n], bps, plan);
        } else if bps < 32 {
            analyzer.analyze(&side32[..n], bps + 1, plan);
        } else {
            analyzer.analyze(&side64[..n], 33, plan);
        }
    }

    fn estimate_slot(&self, slot: usize, n: usize, bps: u32) -> u64 {
        if slot < self.planes.len() {
            fixed::estimate(&self.planes[slot][..n])
        } else if slot == 2 {
            fixed::estimate(&self.mid[..n])
        } else if bps < 32 {
            fixed::estimate(&self.side32[..n])
        } else {
            fixed::estimate(&self.side64[..n])
        }
    }

    /// Picks the channel assignment of a stereo block and plans the two channels it uses.
    fn analyze_stereo(&mut self, n: usize, bps: u32) -> ChannelAssignment {
        self.mid.clear();
        self.side32.clear();
        self.side64.clear();
        for (&l, &r) in self.planes[0][..n].iter().zip(&self.planes[1][..n]) {
            let (l, r) = (i64::from(l), i64::from(r));
            self.mid.push(((l + r) >> 1) as i32);
            if bps < 32 {
                self.side32.push((l - r) as i32);
            } else {
                self.side64.push(l - r);
            }
        }
        // Slots: left, right, mid, side.
        let modes = [
            (ChannelAssignment::Independent(2), [0, 1]),
            (ChannelAssignment::LeftSide, [0, 3]),
            (ChannelAssignment::SideRight, [3, 1]),
            (ChannelAssignment::MidSide, [2, 3]),
        ];
        #[cfg(test)]
        if let Some(mode) = self.force {
            let pair = modes.iter().find(|m| m.0 == mode).map_or([0, 1], |m| m.1);
            for slot in pair {
                self.analyze_slot(slot, n, bps);
            }
            return mode;
        }
        let pick = |cost: &[u64; 4]| {
            let mut best = 0;
            for m in 1..modes.len() {
                let total = |i: usize| cost[modes[i].1[0]] + cost[modes[i].1[1]];
                if total(m) < total(best) {
                    best = m;
                }
            }
            best
        };
        let mut cost = [0u64; 4];
        let best = if self.level.stereo == Stereo::Exhaustive {
            for (slot, c) in cost.iter_mut().enumerate() {
                self.analyze_slot(slot, n, bps);
                *c = self.slots[slot].bits;
            }
            pick(&cost)
        } else {
            for (slot, c) in cost.iter_mut().enumerate() {
                *c = self.estimate_slot(slot, n, bps);
            }
            let best = pick(&cost);
            for slot in modes[best].1 {
                self.analyze_slot(slot, n, bps);
            }
            if best != 0 {
                // The estimate chose; exact bits decide against plain coding.
                let [a, b] = modes[best].1;
                let total = self.slots[a].bits + self.slots[b].bits;
                let plain: u64 = (0..2)
                    .map(|c| {
                        if is_constant(&self.planes[c][..n]) {
                            8 + u64::from(bps)
                        } else {
                            8 + n as u64 * u64::from(bps)
                        }
                    })
                    .sum();
                if total > plain {
                    for c in 0..2 {
                        self.analyzer
                            .trivial(&self.planes[c][..n], bps, &mut self.slots[c]);
                    }
                    return modes[0].0;
                }
            }
            best
        };
        modes[best].0
    }

    fn write_frame(&mut self, n: usize, out: &mut Vec<u8>) {
        let bps = u32::from(self.config.bits_per_sample);
        let channels = usize::from(self.config.channels);
        let assignment = if channels == 2 && self.level.stereo != Stereo::Off {
            self.analyze_stereo(n, bps)
        } else {
            for c in 0..channels {
                self.analyze_slot(c, n, bps);
            }
            ChannelAssignment::Independent(self.config.channels)
        };
        let start = out.len();
        frame::write_header(
            &FrameHeader {
                variable_block_size: false,
                block_size: n as u32,
                sample_rate: Some(self.config.sample_rate),
                channels: assignment,
                bits_per_sample: Some(self.config.bits_per_sample),
                number: self.frame_number,
                header_len: 0,
            },
            out,
        );
        let mut sink = BitSink::new(out);
        let slots: [usize; 8] = match assignment {
            ChannelAssignment::Independent(_) => [0, 1, 2, 3, 4, 5, 6, 7],
            ChannelAssignment::LeftSide => [0, 3, 0, 0, 0, 0, 0, 0],
            ChannelAssignment::SideRight => [3, 1, 0, 0, 0, 0, 0, 0],
            ChannelAssignment::MidSide => [2, 3, 0, 0, 0, 0, 0, 0],
        };
        for &slot in &slots[..usize::from(assignment.channels())] {
            let plan = &self.slots[slot];
            if slot < channels {
                write::write_subframe(&mut sink, plan, &self.planes[slot][..n]);
            } else if slot == 2 {
                write::write_subframe(&mut sink, plan, &self.mid[..n]);
            } else if bps < 32 {
                write::write_subframe(&mut sink, plan, &self.side32[..n]);
            } else {
                write::write_subframe(&mut sink, plan, &self.side64[..n]);
            }
        }
        sink.finish();
        let crc = crate::crc::crc16(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    #[cfg(test)]
    fn force_stereo(&mut self, mode: ChannelAssignment) {
        self.force = Some(mode);
    }

    fn info(&self) -> StreamInfoBlock {
        StreamInfoBlock {
            min_block_size: self.block as u16,
            max_block_size: self.block as u16,
            min_frame_size: 0,
            max_frame_size: 0,
            sample_rate: self.config.sample_rate,
            channels: self.config.channels,
            bits_per_sample: self.config.bits_per_sample,
            total_samples: 0,
            md5: [0; 16],
        }
    }

    /// Provisional: block sizes and format only; frame sizes, total and MD5 are zero
    /// (unknown) until [`finish`](Self::finish).
    pub fn stream_info(&self) -> StreamInfoBlock {
        self.done.unwrap_or_else(|| self.info())
    }

    /// Ends the stream and returns the final STREAMINFO (minimum and maximum frame size,
    /// total samples, MD5). Further blocks are refused; calling it again returns the same.
    pub fn finish(&mut self) -> StreamInfoBlock {
        if let Some(info) = self.done {
            return info;
        }
        let mut info = self.info();
        if self.frame_number > 0 {
            info.min_frame_size = self.min_frame;
            info.max_frame_size = self.max_frame;
        }
        info.total_samples = self.total;
        info.md5 = std::mem::take(&mut self.md5).finalize();
        self.done = Some(info);
        info
    }
}

#[cfg(test)]
mod tests;
