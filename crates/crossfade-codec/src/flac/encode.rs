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
use crate::md5::Md5;
use crossfade_core::{AudioFrame, AudioSpec, Encoder, Error, Packet, SampleFormat, Samples};
use flacenc::bitsink::MemSink;
use flacenc::component::BitRepr;
use flacenc::config;
use flacenc::error::Verified;
use flacenc::source::{Fill, FrameBuf};
use std::collections::VecDeque;

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

    /// flacenc never turns verbatim coding off, so with everything else off it is all that is left.
    fn verbatim_config() -> Result<Verified<config::Encoder>, Error> {
        use flacenc::error::Verify;
        let mut c = config::Encoder::default();
        c.multithread = false;
        c.stereo_coding.use_leftside = false;
        c.stereo_coding.use_rightside = false;
        c.stereo_coding.use_midside = false;
        c.subframe_coding.use_constant = false;
        c.subframe_coding.use_fixed = false;
        c.subframe_coding.use_lpc = false;
        c.into_verified().map_err(|(_, e)| Error::Malformed {
            what: format!("flac encoder settings: {e}"),
        })
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
    rate: u32,
    channels: u16,
    format: SampleFormat,
    bits: u8,
    block: usize,
    config: Verified<config::Encoder>,
    /// Verbatim subframes only: always valid, for a block `config` mis-codes.
    fallback: Verified<config::Encoder>,
    /// flacenc's own header record; it only has to agree with the frames it is asked to code.
    stream_info: flacenc::component::StreamInfo,
    framebuf: FrameBuf,
    /// Interleaved samples of the block being filled; never more than one block.
    buf: Vec<i32>,
    /// The block's samples as MD5 input, reused.
    md5_bytes: Vec<u8>,
    md5: Md5,
    total_samples: u64,
    next_frame: usize,
    min_frame_size: u32,
    max_frame_size: u32,
    pending: VecDeque<Packet>,
    drained: bool,
    failed: Option<Error>,
    extradata: [u8; StreamInfoBlock::LEN],
}

fn malformed(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Malformed {
        what: format!("{what}: {e}"),
    }
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
    let framebuf = FrameBuf::with_size(usize::from(channels), level.block)
        .map_err(|e| malformed("flac encoder buffer", e))?;
    let stream_info = flacenc::component::StreamInfo::new(
        rate as usize,
        usize::from(channels),
        usize::from(bits),
    )
    .map_err(|e| malformed("flac stream parameters", e))?;
    let mut enc = FlacEncoder {
        rate,
        channels,
        format: spec.format(),
        bits,
        block: level.block,
        config: level.config()?,
        fallback: Level::verbatim_config()?,
        stream_info,
        framebuf,
        buf: Vec::with_capacity(level.block * usize::from(channels)),
        md5_bytes: Vec::new(),
        md5: Md5::new(),
        total_samples: 0,
        next_frame: 0,
        min_frame_size: u32::MAX,
        max_frame_size: 0,
        pending: VecDeque::new(),
        drained: false,
        failed: None,
        extradata: [0; StreamInfoBlock::LEN],
    };
    enc.extradata = enc.header(0, [0; 16], 0, 0).write();
    Ok(Box::new(enc))
}

impl FlacEncoder {
    fn header(&self, total: u64, md5: [u8; 16], min_frame: u32, max_frame: u32) -> StreamInfoBlock {
        StreamInfoBlock {
            // The short last block must not lower the minimum: readers reject min != max
            // for a stream that is not variable-blocked.
            min_block_size: self.block as u16,
            max_block_size: self.block as u16,
            min_frame_size: min_frame,
            max_frame_size: max_frame,
            sample_rate: self.rate,
            channels: self.channels as u8,
            bits_per_sample: self.bits,
            total_samples: total,
            md5,
        }
    }

    /// Takes samples until the iterator ends, coding each block as it fills.
    fn push(&mut self, mut samples: impl Iterator<Item = i32>) -> Result<(), Error> {
        let cap = self.block * usize::from(self.channels);
        loop {
            let before = self.buf.len();
            self.buf.extend(samples.by_ref().take(cap - before));
            if self.buf.len() == before {
                return Ok(());
            }
            if self.buf.len() == cap {
                self.flush_block()?;
            }
        }
    }

    fn flush_block(&mut self) -> Result<(), Error> {
        let channels = usize::from(self.channels);
        let frames = self.buf.len() / channels;
        if frames == 0 {
            return Ok(());
        }
        let width = usize::from(self.bits).div_ceil(8);
        self.md5_bytes.clear();
        for s in &self.buf {
            self.md5_bytes.extend_from_slice(&s.to_le_bytes()[..width]);
        }
        self.md5.update(&self.md5_bytes);
        self.framebuf
            .fill_interleaved(&self.buf)
            .map_err(|e| malformed("flac encoder input", e))?;
        self.buf.clear();
        // Raw samples (one more bit on a side channel) plus the headers bound any sane frame.
        let limit = 16 + 2 + channels * (1 + (frames * (usize::from(self.bits) + 1)).div_ceil(8));
        // `None`: the frame is over the limit, found from its size before any of it is written.
        let code = |config: &Verified<config::Encoder>| -> Result<Option<Vec<u8>>, Error> {
            let frame = flacenc::encode_fixed_size_frame(
                config,
                &self.framebuf,
                self.next_frame,
                &self.stream_info,
            )
            .map_err(|e| malformed("flac encoder", e))?;
            if frame.count_bits().div_ceil(8) > limit {
                return Ok(None);
            }
            let mut sink = MemSink::<u8>::with_capacity(frame.count_bits());
            frame
                .write(&mut sink)
                .map_err(|e| malformed("flac frame", e))?;
            Ok(Some(sink.into_inner()))
        };
        // flacenc picks a subframe by an estimated size and, with residuals near full scale, can
        // write a Rice parameter above 14 as if it fitted: valid but enormous (hundreds of MB).
        let data = match code(&self.config)? {
            Some(data) => data,
            None => code(&self.fallback)?.ok_or_else(|| Error::Malformed {
                what: format!("flac encoder: a {frames}-sample frame came out over {limit} bytes"),
            })?,
        };
        let size = u32::try_from(data.len()).map_err(|_| Error::Overflow)?;
        self.min_frame_size = self.min_frame_size.min(size);
        self.max_frame_size = self.max_frame_size.max(size);
        self.pending.push_back(Packet {
            stream: 0,
            pts: self.total_samples as i64,
            duration: frames as u64,
            data,
        });
        self.total_samples += frames as u64;
        self.next_frame += 1;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), Error> {
        self.flush_block()?;
        let min = if self.next_frame == 0 {
            0
        } else {
            self.min_frame_size
        };
        self.extradata = self
            .header(
                self.total_samples,
                self.md5.clone().finalize(),
                min,
                self.max_frame_size,
            )
            .write();
        Ok(())
    }
}

impl Encoder for FlacEncoder {
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        if frame.samples.format() != self.format
            || frame.sample_rate != self.rate
            || frame.layout.channels() != self.channels
        {
            return Err(unsupported(format!(
                "frame is {} Hz x {} ch {} but the encoder is {} Hz x {} ch {}",
                frame.sample_rate,
                frame.layout.channels(),
                frame.samples.format().name(),
                self.rate,
                self.channels,
                self.format.name()
            )));
        }
        if self.drained {
            return Err(unsupported("send_frame after drain"));
        }
        if !frame
            .samples
            .len()
            .is_multiple_of(usize::from(self.channels))
        {
            return Err(Error::Malformed {
                what: format!(
                    "{} samples is not a whole number of {}-channel frames",
                    frame.samples.len(),
                    self.channels
                ),
            });
        }
        match &frame.samples {
            Samples::U8(v) => self.push(v.iter().map(|&x| i32::from(x) - 128)),
            Samples::S16(v) => self.push(v.iter().map(|&x| i32::from(x))),
            Samples::S24(v) => {
                if v.iter().any(|x| !(-(1 << 23)..1 << 23).contains(x)) {
                    return Err(Error::Malformed {
                        what: "24-bit sample outside -2^23..2^23-1".into(),
                    });
                }
                self.push(v.iter().copied())
            }
            Samples::S32(_) | Samples::F32(_) | Samples::F64(_) => {
                unreachable!("the constructor refused these formats")
            }
        }
    }

    fn receive_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if let Some(e) = self.failed.take() {
            return Err(e);
        }
        match self.pending.pop_front() {
            Some(p) => {
                *out = p;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn drain(&mut self) {
        if self.drained {
            return;
        }
        self.drained = true;
        // A failure here cannot be returned; the next `receive_packet` reports it.
        if let Err(e) = self.finish() {
            self.failed = Some(e);
        }
    }

    fn extradata(&self) -> &[u8] {
        &self.extradata
    }
}

#[cfg(test)]
mod tests;
