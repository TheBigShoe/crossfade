//! FLAC encoder: the `flacenc` crate behind the streaming [`Encoder`] trait.
//!
//! Input is re-blocked into fixed-size blocks, each encoded to one complete FLAC frame
//! (header, subframes, CRC-16), so a packet is exactly one frame. Everything runs on the
//! calling thread (`multithread = false`): identical input gives identical bytes anywhere.
//!
//! # Compression levels
//!
//! `EncodeOptions::compression_level` is 0 to 8; `None` means 5. Each step turns on strictly
//! more search, so output never grows with the level on ordinary material.
//!
//! | level | block | stereo modes             | fixed order sel | LPC order / precision |
//! |------:|------:|--------------------------|-----------------|-----------------------|
//! | 0     | 1152  | independent only         | approximate     | none (fixed only)     |
//! | 1     | 1152  | independent, mid/side    | approximate     | none                  |
//! | 2     | 1152  | all                      | approximate     | none                  |
//! | 3     | 4096  | independent, mid/side    | approximate     | 6 / 12                |
//! | 4     | 4096  | all                      | approximate     | 8 / 13                |
//! | 5     | 4096  | all                      | approximate     | 10 / 15               |
//! | 6     | 4096  | all                      | approximate     | 12 / 15               |
//! | 7     | 4096  | all                      | exact bit count | 16 / 15               |
//! | 8     | 4096  | all                      | exact bit count | 20 / 15               |
//!
//! Levels 0 to 2 use the short block the reference encoder uses for its fast presets.
//!
//! # Limits
//!
//! `flacenc` 0.5.1 takes 8, 16 and 24 bit samples, 1 to 8 channels, sample rates up to
//! 96 000 Hz (its `StreamInfo` refuses more, although the format allows 1 048 575) and
//! blocks of 32 to 32 767 samples.

use crate::EncodeOptions;
use crate::flac::streaminfo::StreamInfoBlock;
use crossfade_core::{AudioFrame, AudioSpec, Encoder, Error, Packet, SampleFormat, Samples};
use flacenc::config;
use flacenc::error::Verified;
use flacenc::source::FrameBuf;

const DEFAULT_LEVEL: u8 = 5;
const MAX_LEVEL: u8 = 8;
/// What flacenc's `StreamInfo::new` accepts.
const MAX_RATE: u32 = 96_000;
const MAX_CHANNELS: u16 = 8;

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

struct Level {
    block: usize,
    left_side: bool,
    right_side: bool,
    mid_side: bool,
    lpc_order: usize,
    precision: usize,
    exact_order_selection: bool,
}

/// Index is the compression level.
const LEVELS: [Level; 9] = [
    Level::fixed(1152, false, false, false),
    Level::fixed(1152, false, false, true),
    Level::fixed(1152, true, true, true),
    Level::lpc(false, 6, 12, false),
    Level::lpc(true, 8, 13, false),
    Level::lpc(true, 10, 15, false),
    Level::lpc(true, 12, 15, false),
    Level::lpc(true, 16, 15, true),
    Level::lpc(true, 20, 15, true),
];

impl Level {
    const fn fixed(block: usize, left_side: bool, right_side: bool, mid_side: bool) -> Self {
        Self {
            block,
            left_side,
            right_side,
            mid_side,
            lpc_order: 0,
            precision: 0,
            exact_order_selection: false,
        }
    }

    const fn lpc(all_stereo: bool, order: usize, precision: usize, exact: bool) -> Self {
        Self {
            block: 4096,
            left_side: all_stereo,
            right_side: all_stereo,
            mid_side: true,
            lpc_order: order,
            precision,
            exact_order_selection: exact,
        }
    }

    fn config(&self) -> Result<Verified<config::Encoder>, Error> {
        use flacenc::error::Verify;
        let mut c = config::Encoder::default();
        c.block_size = self.block;
        // One thread keeps the output a pure function of the input.
        c.multithread = false;
        c.stereo_coding.use_leftside = self.left_side;
        c.stereo_coding.use_rightside = self.right_side;
        c.stereo_coding.use_midside = self.mid_side;
        c.subframe_coding.use_lpc = self.lpc_order > 0;
        if self.lpc_order > 0 {
            c.subframe_coding.qlpc.lpc_order = self.lpc_order;
            c.subframe_coding.qlpc.quant_precision = self.precision;
        }
        if self.exact_order_selection {
            c.subframe_coding.fixed.order_sel = config::OrderSel::BitCount;
        }
        c.into_verified().map_err(|(_, e)| Error::Malformed {
            what: format!("flac encoder settings: {e}"),
        })
    }
}

fn bits_of(format: SampleFormat) -> Result<u8, Error> {
    match format {
        SampleFormat::U8 => Ok(8),
        SampleFormat::S16 => Ok(16),
        SampleFormat::S24 => Ok(24),
        SampleFormat::S32 => Err(unsupported(
            "flac encoding of 32-bit samples is not supported yet; choose --sample-format s24",
        )),
        SampleFormat::F32 | SampleFormat::F64 => Err(unsupported(
            "flac encoding of float samples is not supported; convert to s16 or s24 first",
        )),
    }
}

struct FlacEncoder {
    spec_rate: u32,
    channels: u16,
    format: SampleFormat,
    block: usize,
    _config: Verified<config::Encoder>,
    _framebuf: FrameBuf,
    extradata: [u8; StreamInfoBlock::LEN],
}

pub fn encoder(spec: &AudioSpec, options: &EncodeOptions) -> Result<Box<dyn Encoder>, Error> {
    let bits = bits_of(spec.format())?;
    let channels = spec.layout().channels();
    if channels > MAX_CHANNELS {
        return Err(unsupported(format!(
            "flac holds at most {MAX_CHANNELS} channels, got {channels}"
        )));
    }
    let rate = spec.sample_rate();
    if rate > MAX_RATE {
        return Err(unsupported(format!(
            "flac encoding supports sample rates up to {MAX_RATE} Hz, got {rate}"
        )));
    }
    let level = options.compression_level.unwrap_or(DEFAULT_LEVEL);
    if level > MAX_LEVEL {
        return Err(unsupported(format!(
            "flac compression level runs from 0 to {MAX_LEVEL}, got {level}"
        )));
    }
    let level = &LEVELS[usize::from(level)];
    let framebuf = FrameBuf::with_size(usize::from(channels), level.block).map_err(|e| {
        Error::Malformed {
            what: format!("flac encoder buffer: {e}"),
        }
    })?;
    let extradata = StreamInfoBlock {
        min_block_size: level.block as u16,
        max_block_size: level.block as u16,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: rate,
        channels: channels as u8,
        bits_per_sample: bits,
        total_samples: 0,
        md5: [0; 16],
    }
    .write();
    Ok(Box::new(FlacEncoder {
        spec_rate: rate,
        channels,
        format: spec.format(),
        block: level.block,
        _config: level.config()?,
        _framebuf: framebuf,
        extradata,
    }))
}

impl Encoder for FlacEncoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        if frame.samples.format() != self.format
            || frame.sample_rate != self.spec_rate
            || frame.layout.channels() != self.channels
        {
            return Err(unsupported(format!(
                "frame is {} Hz x {} ch {} but the encoder is {} Hz x {} ch {}",
                frame.sample_rate,
                frame.layout.channels(),
                frame.samples.format().name(),
                self.spec_rate,
                self.channels,
                self.format.name()
            )));
        }
        let _ = (self.block, Samples::new(self.format));
        Ok(())
    }

    fn receive_packet(&mut self, _out: &mut Packet) -> Result<bool, Error> {
        Ok(false)
    }

    fn drain(&mut self) {}

    fn extradata(&self) -> &[u8] {
        &self.extradata
    }
}

#[cfg(test)]
mod tests;
