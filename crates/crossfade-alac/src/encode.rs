//! The packet encoder: one frame of every channel in, one packet out.

use crate::bits::BitWriter;
use crate::decode::{CPE, END, SCE};
use crate::{Config, Error, Result};

/// The frame length Apple's encoder uses.
pub const DEFAULT_FRAME_LENGTH: u32 = 4096;

/// What a stream is to be: the encoder's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncoderConfig {
    pub sample_rate: u32,
    /// 1 to 8, in ALAC's order for the count (see [`wave_layout`](crate::wave_layout)).
    pub channels: u8,
    /// 16, 20, 24 or 32.
    pub bit_depth: u8,
    /// Sample frames in every packet but a shorter last one, 1 to 2^20.
    pub frame_length: u32,
}

impl EncoderConfig {
    /// A stream with Apple's frame length.
    pub fn new(sample_rate: u32, channels: u8, bit_depth: u8) -> Self {
        Self {
            sample_rate,
            channels,
            bit_depth,
            frame_length: DEFAULT_FRAME_LENGTH,
        }
    }
}

/// Encodes the packets of one stream, in order.
#[derive(Debug, Clone)]
pub struct Encoder {
    config: Config,
    bytes: u64,
    packets: u64,
    /// A packet shorter than the frame length was written: it must be the last.
    ended: bool,
}

impl Encoder {
    pub fn new(settings: EncoderConfig) -> Result<Self> {
        if settings.sample_rate == 0 {
            return Err(Error::InvalidConfig("sample rate is 0"));
        }
        // The rice coder's tuning is Apple's, as afconvert writes it.
        let config = Config {
            frame_length: settings.frame_length,
            compatible_version: 0,
            bit_depth: settings.bit_depth,
            pb: 40,
            mb: 10,
            kb: 14,
            channels: settings.channels,
            max_run: 255,
            max_frame_bytes: 0,
            avg_bit_rate: 0,
            sample_rate: settings.sample_rate,
        };
        config.check()?;
        Ok(Self {
            config,
            bytes: 0,
            packets: 0,
            ended: false,
        })
    }

    /// The stream's ALACSpecificConfig, its byte counts final once the last packet is written.
    /// The average bit rate counts every packet as a whole frame long, as `afconvert` does.
    pub fn config(&self) -> Config {
        let mut c = self.config;
        if self.packets > 0 {
            let frames = self.packets * u64::from(c.frame_length);
            let rate = self.bytes * 8 * u64::from(c.sample_rate) / frames;
            c.avg_bit_rate = u32::try_from(rate).unwrap_or(u32::MAX);
        }
        c
    }

    /// Encodes one packet into `out` (cleared first) from interleaved samples in ALAC's
    /// channel order, right-justified at the stream's bit depth: the frame length's worth of
    /// sample frames, or fewer for the last packet.
    pub fn encode(&mut self, pcm: &[i32], out: &mut Vec<u8>) -> Result<()> {
        out.clear();
        let channels = usize::from(self.config.channels);
        if self.ended {
            return Err(Error::InvalidInput("a packet after a short one"));
        }
        if pcm.is_empty() {
            return Err(Error::InvalidInput("no samples"));
        }
        if !pcm.len().is_multiple_of(channels) {
            return Err(Error::InvalidInput(
                "sample count is not a multiple of the channel count",
            ));
        }
        let frames = pcm.len() / channels;
        if frames > self.config.frame_length as usize {
            return Err(Error::InvalidInput("more samples than one frame length"));
        }
        let depth = u32::from(self.config.bit_depth);
        if depth < 32 {
            let (min, max) = (-(1i32 << (depth - 1)), (1i32 << (depth - 1)) - 1);
            if pcm.iter().any(|s| !(min..=max).contains(s)) {
                return Err(Error::InvalidInput("sample out of range for the bit depth"));
            }
        }

        let mut w = BitWriter::new(out);
        let mut ch = 0;
        for &tag in elements(self.config.channels) {
            let width = if tag == CPE { 2 } else { 1 };
            w.put(3, tag);
            w.put(4, 0); // element instance tag
            self.escape(&mut w, pcm, ch, width, frames);
            ch += width;
        }
        w.put(3, END);
        w.align();

        self.ended = frames < self.config.frame_length as usize;
        self.packets += 1;
        self.bytes += out.len() as u64;
        let len = u32::try_from(out.len()).unwrap_or(u32::MAX);
        self.config.max_frame_bytes = self.config.max_frame_bytes.max(len);
        Ok(())
    }

    /// The element header after the tag: nothing shifted, and the length when it is short.
    fn header(&self, w: &mut BitWriter<'_>, frames: usize, escape: bool) {
        let partial = frames < self.config.frame_length as usize;
        w.put(12, 0);
        w.put(4, u32::from(partial) << 3 | u32::from(escape));
        if partial {
            w.put(32, frames as u32);
        }
    }

    /// An uncompressed element: every sample at the full depth, interleaved within a pair.
    fn escape(&self, w: &mut BitWriter<'_>, pcm: &[i32], ch: usize, width: usize, frames: usize) {
        self.header(w, frames, true);
        let depth = u32::from(self.config.bit_depth);
        let channels = usize::from(self.config.channels);
        for frame in pcm.chunks_exact(channels).take(frames) {
            for &s in &frame[ch..ch + width] {
                w.put(depth, s as u32);
            }
        }
    }
}

/// The elements Apple's encoder writes for each channel count, in ALAC's channel order. The
/// LFE channel of 5.1, 6.1 and 7.1 goes in a single channel element, as `afconvert` writes it.
fn elements(channels: u8) -> &'static [u32] {
    match channels {
        1 => &[SCE],
        2 => &[CPE],
        3 => &[SCE, CPE],
        4 => &[SCE, CPE, SCE],
        5 => &[SCE, CPE, CPE],
        6 => &[SCE, CPE, CPE, SCE],
        7 => &[SCE, CPE, CPE, SCE, SCE],
        _ => &[SCE, CPE, CPE, CPE, SCE],
    }
}
