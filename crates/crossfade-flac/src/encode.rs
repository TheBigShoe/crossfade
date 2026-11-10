//! A FLAC encoder (RFC 9639): interleaved samples in, one frame per block out.

mod analyze;
mod bitsink;
mod fixed;
mod rice;
mod write;

use crate::crc::crc16;
use crate::error::{Error, Result};
use crate::frame::{self, ChannelAssignment, FrameHeader};
use crate::md5::Md5;
use crate::streaminfo::StreamInfoBlock;
use analyze::{Analyzer, Level, Plan};
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
    slots: Vec<Plan>,
    analyzer: Analyzer,
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
            slots: (0..channels).map(|_| Plan::new(level.block)).collect(),
            analyzer: Analyzer::new(level),
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

    fn write_frame(&mut self, n: usize, out: &mut Vec<u8>) {
        let bps = u32::from(self.config.bits_per_sample);
        let channels = usize::from(self.config.channels);
        for c in 0..channels {
            self.analyzer
                .analyze(&self.planes[c][..n], bps, &mut self.slots[c]);
        }
        let start = out.len();
        frame::write_header(
            &FrameHeader {
                variable_block_size: false,
                block_size: n as u32,
                sample_rate: Some(self.config.sample_rate),
                channels: ChannelAssignment::Independent(self.config.channels),
                bits_per_sample: Some(self.config.bits_per_sample),
                number: self.frame_number,
                header_len: 0,
            },
            out,
        );
        let mut sink = BitSink::new(out);
        for c in 0..channels {
            write::write_subframe(&mut sink, &self.slots[c], &self.planes[c][..n]);
        }
        sink.finish();
        let crc = crc16(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
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
