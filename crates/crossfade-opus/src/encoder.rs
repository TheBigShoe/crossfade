//! The Opus encoder: for now CELT-only at full band, constant or variable bitrate, one frame of 2.5 to
//! 20 ms per packet (TOC configurations 28 to 31, code 0). The frame is
//! [`CeltEncoder`]'s, a port of libopus's `celt_encode_with_ec`.

use crate::celt::{CeltEncoder, CeltFrame};
use crate::error::{Error, Result};
use crate::head::{ChannelMapping, OpusHead};
use crate::range_encoder::RangeEncoder;

/// The lowest and highest bitrates of the format, in bits per second.
const BITRATES: std::ops::RangeInclusive<u32> = 6_000..=510_000;
/// The first CELT full-band configuration (2.5 ms; 5, 10 and 20 ms follow).
const CONFIG_CELT_FB: u8 = 28;
/// The first CELT narrow-band configuration, for an LFE stream.
const CONFIG_CELT_NB: u8 = 16;

/// An Opus encoder for one mono or stereo stream at 48 kHz.
#[derive(Debug, Clone)]
pub struct Encoder {
    celt: CeltEncoder,
    channels: usize,
    bitrate: u32,
    frame_size: usize,
    vbr: bool,
    lfe: bool,
}

impl Encoder {
    /// An encoder for `channels` (1 or 2) channels: 20 ms frames at 64 kb/s a channel.
    pub fn new(channels: usize) -> Result<Self> {
        if !(1..=2).contains(&channels) {
            return Err(Error::BadArgument("an Opus stream has one or two channels"));
        }
        Ok(Self {
            celt: CeltEncoder::new(channels),
            channels,
            bitrate: 64_000 * channels as u32,
            frame_size: 960,
            vbr: false,
            lfe: false,
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The bitrate in bits per second, TOC byte included.
    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }

    /// Sets the bitrate, 6,000 to 510,000 bits per second for the whole stream.
    pub fn set_bitrate(&mut self, bits_per_second: u32) -> Result<()> {
        if !BITRATES.contains(&bits_per_second) {
            return Err(Error::BadArgument("an Opus bitrate is 6 to 510 kb/s"));
        }
        self.bitrate = bits_per_second;
        Ok(())
    }

    /// Whether the bitrate is variable.
    pub fn vbr(&self) -> bool {
        self.vbr
    }

    /// A variable bitrate spends around [`bitrate`](Self::bitrate) on average: more on what
    /// is hard to code, two bytes on silence. Off (the default), every packet has the same
    /// size.
    pub fn set_vbr(&mut self, vbr: bool) {
        self.vbr = vbr;
    }

    /// Codes the stream as a low-frequency effects channel (`OPUS_SET_LFE`, as a surround
    /// encoder sets it): narrow band, nearly all of its bits in the lowest band.
    pub(crate) fn set_lfe(&mut self, lfe: bool) {
        self.lfe = lfe;
        self.celt.set_lfe(lfe);
    }

    /// Samples per channel that [`encode`](Self::encode) takes.
    pub fn frame_size(&self) -> usize {
        self.frame_size
    }

    /// Sets the frame to 120, 240, 480 or 960 samples (2.5 to 20 ms).
    pub fn set_frame_size(&mut self, samples: usize) -> Result<()> {
        if ![120, 240, 480, 960].contains(&samples) {
            return Err(Error::BadArgument(
                "an Opus frame here is 2.5, 5, 10 or 20 ms",
            ));
        }
        self.frame_size = samples;
        Ok(())
    }

    /// Samples by which the decoded signal trails the input: an `OpusHead`'s pre-skip.
    pub fn lookahead(&self) -> usize {
        crate::celt::OVERLAP
    }

    /// The range the last packet ended on (`OPUS_GET_FINAL_RANGE`), which a decoder of that
    /// packet must end on too.
    pub fn final_range(&self) -> u32 {
        self.celt.final_range()
    }

    /// Forgets the stream; the settings stay.
    pub fn reset(&mut self) {
        self.celt.reset();
    }

    /// The identification header of the stream this encoder writes, for an input that was
    /// at `input_sample_rate` Hz before it was brought to 48 kHz (0: unspecified).
    pub fn head(&self, input_sample_rate: u32) -> OpusHead {
        let channels = self.channels as u8;
        OpusHead {
            version: 1,
            channels,
            pre_skip: self.lookahead() as u16,
            input_sample_rate,
            output_gain: 0,
            mapping: ChannelMapping {
                family: 0,
                streams: 1,
                coupled: channels - 1,
                table: (0..channels).collect(),
            },
        }
    }

    /// The bytes of one packet at the bitrate, TOC included.
    fn packet_bytes(&self) -> usize {
        let bits = self.bitrate as usize * self.frame_size / 48_000;
        bits.div_ceil(8).clamp(3, 1276)
    }

    /// Encodes one frame, `frame_size * channels` interleaved samples at 48 kHz with full
    /// scale 1.0, and appends its packet to `out`. On an error nothing is appended.
    pub fn encode(&mut self, pcm: &[f32], out: &mut Vec<u8>) -> Result<()> {
        if pcm.len() != self.frame_size * self.channels {
            return Err(Error::BadArgument("an Opus frame of the wrong length"));
        }
        let lm = self.frame_size.trailing_zeros() - 120u32.trailing_zeros();
        let frame = CeltFrame {
            frame_size: self.frame_size,
            start_band: 0,
            end_band: if self.lfe { 13 } else { 21 },
            stream_channels: self.channels,
            accumulate: false,
        };
        // A variable bitrate may take up to the format's largest frame.
        let bytes = if self.vbr {
            1275
        } else {
            self.packet_bytes() - 1
        };
        self.celt.set_vbr(self.vbr.then_some(self.bitrate as i32));
        let mut renc = RangeEncoder::new(bytes);
        self.celt.encode(pcm, &frame, &mut renc)?;
        let data = renc
            .done()
            .ok_or(Error::Corrupt("the frame did not fit its bytes"))?;
        let config = if self.lfe {
            CONFIG_CELT_NB
        } else {
            CONFIG_CELT_FB
        };
        let toc = ((config + lm as u8) << 3) | (u8::from(self.channels == 2) << 2);
        out.push(toc);
        out.extend_from_slice(&data);
        Ok(())
    }
}
