//! A FLAC encoder (RFC 9639): interleaved samples in, one frame per block out.

mod analyze;
mod bitsink;
mod fixed;
mod lpc;
mod rice;
mod write;

use crate::crc::crc16;
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
    first_len: usize,
    short_seen: bool,
    done: Option<StreamInfoBlock>,
    md5: Md5,
    md5_bytes: Vec<u8>,
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
            first_len: 0,
            short_seen: false,
            done: None,
            md5: Md5::new(),
            md5_bytes: Vec::with_capacity(level.block * channels * 4),
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
            analyzer: Analyzer::new(level),
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
        let bps = u32::from(self.config.bits_per_sample);
        if bps < 32 {
            let (lo, hi) = (-(1i32 << (bps - 1)), (1i32 << (bps - 1)) - 1);
            if interleaved.iter().any(|&s| s < lo || s > hi) {
                return Err(malformed("sample outside the configured bit depth"));
            }
        }
        if self.frame_number >= 1 << 31 || self.total + n as u64 >= 1 << 36 {
            return Err(unsupported("stream is longer than FLAC can number"));
        }

        for (c, plane) in self.planes.iter_mut().enumerate() {
            plane.clear();
            plane.extend(interleaved[c..].iter().step_by(channels).copied());
        }
        self.hash(interleaved);

        let start = out.len();
        self.write_frame(n, out);
        let len = (out.len() - start) as u32;
        self.min_frame = self.min_frame.min(len);
        self.max_frame = self.max_frame.max(len);
        if self.frame_number == 0 {
            self.first_len = n;
        }
        self.frame_number += 1;
        self.total += n as u64;
        self.short_seen = n < self.block;
        Ok(())
    }

    /// The MD5 is over the samples as little-endian integers in the smallest whole number
    /// of bytes the depth fits.
    fn hash(&mut self, interleaved: &[i32]) {
        let width = usize::from(self.config.bits_per_sample).div_ceil(8);
        self.md5_bytes.clear();
        for &s in interleaved {
            self.md5_bytes.extend_from_slice(&s.to_le_bytes()[..width]);
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
        let crc = crc16(&out[start..]);
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
        if self.frame_number == 1 {
            // A stream of one short block: the block size is that block's length.
            info.min_block_size = self.first_len as u16;
            info.max_block_size = self.first_len as u16;
        }
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
