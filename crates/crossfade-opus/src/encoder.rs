//! The Opus encoder, CELT-only for now: the part of `src/opus_encoder.c`
//! (`opus_encode_native`, `opus_encode_frame_native`) a CELT stream needs, with
//! `OPUS_APPLICATION_AUDIO`'s signal path (the 3 Hz DC filter, the 4 ms delay that keeps
//! the lookahead the same once SILK and hybrid join), the rate-dependent stereo and
//! bandwidth decisions, CBR and VBR, the TOC byte and code-0 packets.

use crate::celt::{CELT_DELAY, CeltEncodeFrame, CeltEncoder, OVERLAP, SilkInfo, WINDOW};
use crate::error::{Error, Result};
use crate::range_enc::RangeEncoder;

/// Samples per channel of the encoder's delay line (`encoder_buffer`, 10 ms).
const ENCODER_BUFFER: usize = 480;
/// `delay_compensation` (4 ms: SILK's look-ahead, its resamplers and stereo prediction).
const DELAY_COMPENSATION: usize = 192;
/// The reference's default complexity.
const COMPLEXITY: i32 = 9;
/// How much of a voice signal the encoder assumes without the tonality analysis (Q7, for
/// `OPUS_APPLICATION_AUDIO`).
const VOICE_EST: i32 = 48;
/// The longest packet: a TOC byte and 1275 bytes of frame.
const MAX_PACKET: usize = 1276;

/// The duration of each packet's single frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum FrameDuration {
    /// 2.5 ms.
    Ms2_5,
    /// 5 ms.
    Ms5,
    /// 10 ms.
    Ms10,
    /// 20 ms.
    #[default]
    Ms20,
}

impl FrameDuration {
    /// Samples per channel at 48 kHz.
    pub fn samples(self) -> usize {
        match self {
            Self::Ms2_5 => 120,
            Self::Ms5 => 240,
            Self::Ms10 => 480,
            Self::Ms20 => 960,
        }
    }
}

/// What an [`Encoder`] makes: 48 kHz input, one or two channels.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncoderConfig {
    /// 1 or 2.
    pub channels: u8,
    /// Bits per second for the whole stream (clamped to 500 to 300000 per channel, as
    /// libopus does). Below 9.6 kb/s at 2.5 ms a packet holds only its TOC byte (and in CBR
    /// a byte of framing): the decoder conceals it, as libopus's "PLC frames".
    pub bitrate: u32,
    /// Variable bitrate (`OPUS_SET_VBR`); false is CBR, every packet the same size.
    pub vbr: bool,
    /// With `vbr`, keep each packet within reach of the bitrate (`OPUS_SET_VBR_CONSTRAINT`).
    /// Unconstrained by default, as `opusenc`.
    pub constrained_vbr: bool,
    pub frame_duration: FrameDuration,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            channels: 2,
            bitrate: 128_000,
            vbr: true,
            constrained_vbr: false,
            frame_duration: FrameDuration::Ms20,
        }
    }
}

/// The audio bandwidth of a packet (`OPUS_BANDWIDTH_*`, in order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Bw {
    Narrow,
    Medium,
    Wide,
    SuperWide,
    Full,
}

impl Bw {
    fn from_index(i: usize) -> Self {
        [
            Self::Narrow,
            Self::Medium,
            Self::Wide,
            Self::SuperWide,
            Self::Full,
        ][i.min(4)]
    }

    /// CELT's end band (`CELT_SET_END_BAND`).
    fn end_band(self) -> usize {
        match self {
            Self::Narrow => 13,
            Self::Medium | Self::Wide => 17,
            Self::SuperWide => 19,
            Self::Full => 21,
        }
    }
}

/// `gen_toc` for a CELT-only frame.
fn celt_toc(frame_rate: usize, bandwidth: Bw, stereo: bool) -> u8 {
    let mut period = 0;
    let mut rate = frame_rate;
    while rate < 400 {
        rate <<= 1;
        period += 1;
    }
    let bw = (bandwidth as u8).saturating_sub(Bw::Medium as u8);
    0x80 | bw << 5 | period << 3 | u8::from(stereo) << 2
}

/// `compute_equiv_rate` for a CELT-only (or not yet decided) stream at complexity 9: the
/// 20 ms VBR rate of the same quality.
fn equiv_rate(bitrate: i32, channels: i32, frame_rate: i32, vbr: bool) -> i32 {
    let mut equiv = bitrate;
    // The overhead of smaller frames.
    if frame_rate > 50 {
        equiv -= (40 * channels + 20) * (frame_rate - 50);
    }
    // CBR costs about 8%.
    if !vbr {
        equiv -= equiv / 12;
    }
    // About 10% between complexity 0 and 10.
    equiv * (90 + COMPLEXITY) / 100
}

/// An Opus encoder: 48 kHz PCM in, one standard Opus packet (CELT-only, code 0) per frame
/// out. A port of libopus 1.5.2's float encoder at complexity 9 without the tonality
/// analysis; see the crate's `COPYING`.
#[derive(Debug, Clone)]
pub struct Encoder {
    config: EncoderConfig,
    channels: usize,
    frame_size: usize,
    /// The user's bitrate, clamped as `OPUS_SET_BITRATE` does.
    bitrate: i32,
    celt: CeltEncoder,
    // From here on, what OPUS_RESET_STATE clears.
    stream_channels: usize,
    bandwidth: Bw,
    auto_bandwidth: Option<Bw>,
    first: bool,
    hybrid_stereo_width_q14: i32,
    /// The DC filter's memory (`hp_mem`: entries 0 and 2 are the channels').
    hp_mem: [f32; 4],
    /// The last [`ENCODER_BUFFER`] samples per channel, interleaved (`delay_buffer`).
    delay_buffer: [f32; ENCODER_BUFFER * 2],
    final_range: u32,
}

impl Encoder {
    /// An encoder for `config`; an error names a value out of range.
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        if !(1..=2).contains(&config.channels) {
            return Err(Error::BadArgument(
                "an Opus encoder has one or two channels",
            ));
        }
        if config.bitrate == 0 {
            return Err(Error::BadArgument("a bitrate of 0"));
        }
        let channels = usize::from(config.channels);
        let bitrate = i64::from(config.bitrate).clamp(500, 300_000 * channels as i64) as i32;
        Ok(Self {
            config: config.clone(),
            channels,
            frame_size: config.frame_duration.samples(),
            bitrate,
            celt: CeltEncoder::new(channels),
            stream_channels: channels,
            bandwidth: Bw::Full,
            auto_bandwidth: None,
            first: true,
            hybrid_stereo_width_q14: 1 << 14,
            hp_mem: [0.0; 4],
            delay_buffer: [0.0; ENCODER_BUFFER * 2],
            final_range: 0,
        })
    }

    /// Samples per channel in each packet.
    pub fn frame_size(&self) -> usize {
        self.frame_size
    }

    /// Samples (at 48 kHz) by which decoded output lags the input: the pre-skip a container
    /// writes (`OPUS_GET_LOOKAHEAD`).
    pub fn lookahead(&self) -> u32 {
        (CELT_DELAY + DELAY_COMPENSATION) as u32
    }

    /// The range coder's final state after the last packet (`OPUS_GET_FINAL_RANGE`): what a
    /// decoder of the packet reports. 0 after a packet with no frame data.
    pub fn final_range(&self) -> u32 {
        self.final_range
    }

    /// Forgets the stream, as if new (`OPUS_RESET_STATE`).
    pub fn reset(&mut self) {
        self.celt.reset();
        self.stream_channels = self.channels;
        self.bandwidth = Bw::Full;
        self.auto_bandwidth = None;
        self.first = true;
        self.hybrid_stereo_width_q14 = 1 << 14;
        self.hp_mem = [0.0; 4];
        self.delay_buffer = [0.0; ENCODER_BUFFER * 2];
        self.final_range = 0;
    }

    /// Encodes exactly [`frame_size`](Self::frame_size) samples per channel, interleaved,
    /// full scale 1.0, into `packet` (cleared first). A frame holding a NaN, an infinity or
    /// absurdly large samples (energy of 1e9 and up) is coded as silence, as libopus does.
    pub fn encode(&mut self, pcm: &[f32], packet: &mut Vec<u8>) -> Result<()> {
        let ch = self.channels;
        let n = self.frame_size;
        if pcm.len() != n * ch {
            return Err(Error::BadArgument(
                "an Opus encoder takes one frame of interleaved samples",
            ));
        }
        packet.clear();
        let vbr = self.config.vbr;
        let frame_rate = (48_000 / n) as i32;
        let mut bitrate = self.bitrate;
        let mut max_data_bytes = MAX_PACKET;
        if !vbr {
            // Times 12 so that the division is exact.
            let frame_rate12 = 12 * 48_000 / n as i32;
            let cbr_bytes =
                ((12 * bitrate / 8 + frame_rate12 / 2) / frame_rate12).min(MAX_PACKET as i32);
            bitrate = cbr_bytes * frame_rate12 * 8 / 12;
            max_data_bytes = cbr_bytes.max(1) as usize;
        }
        if max_data_bytes < 3 || bitrate < 3 * frame_rate * 8 {
            // Too few bits to code anything useful: a packet the decoder conceals.
            let bw = if self.bandwidth == Bw::Medium {
                Bw::Narrow
            } else {
                self.bandwidth
            };
            packet.push(celt_toc(frame_rate as usize, bw, self.stream_channels == 2));
            if !vbr && max_data_bytes == 2 {
                // Code 3, one frame of no bytes: the CBR size.
                packet[0] |= 3;
                packet.push(1);
            }
            self.final_range = 0;
            return Ok(());
        }

        let mut equiv = equiv_rate(bitrate, ch as i32, frame_rate, vbr);
        if ch == 2 {
            // Stereo above about 17 kb/s, with hysteresis.
            let mut threshold = 17_000 + ((VOICE_EST * VOICE_EST * (19_000 - 17_000)) >> 14);
            if self.stream_channels == 2 {
                threshold -= 1000;
            } else {
                threshold += 1000;
            }
            self.stream_channels = if equiv > threshold { 2 } else { 1 };
            equiv = equiv_rate(bitrate, self.stream_channels as i32, frame_rate, vbr);
        }
        self.bandwidth = self.choose_bandwidth(equiv);
        // CELT has no medium band.
        let bandwidth = if self.bandwidth == Bw::Medium {
            Bw::Wide
        } else {
            self.bandwidth
        };

        // opus_encode_frame_native.
        let total_buffer = DELAY_COMPENSATION;
        let mut pcm_buf = [0.0f32; (DELAY_COMPENSATION + 960) * 2];
        let pcm_buf = &mut pcm_buf[..(total_buffer + n) * ch];
        pcm_buf[..total_buffer * ch].copy_from_slice(
            &self.delay_buffer[(ENCODER_BUFFER - total_buffer) * ch..ENCODER_BUFFER * ch],
        );
        self.dc_reject(pcm, &mut pcm_buf[total_buffer * ch..]);
        {
            let new = &mut pcm_buf[total_buffer * ch..];
            let mut sum = 0.0f32;
            for &v in new.iter() {
                sum += v * v;
            }
            // Filters NaNs and signals absurd enough to make NaNs further down.
            if sum.partial_cmp(&1e9) != Some(std::cmp::Ordering::Less) {
                new.fill(0.0);
                self.hp_mem = [0.0; 4];
            }
        }
        if ENCODER_BUFFER > n + total_buffer {
            let keep = ENCODER_BUFFER - n - total_buffer;
            self.delay_buffer.copy_within(ch * n..ch * (n + keep), 0);
            self.delay_buffer[ch * keep..ch * ENCODER_BUFFER].copy_from_slice(pcm_buf);
        } else {
            let from = (n + total_buffer - ENCODER_BUFFER) * ch;
            self.delay_buffer[..ENCODER_BUFFER * ch]
                .copy_from_slice(&pcm_buf[from..from + ENCODER_BUFFER * ch]);
        }
        // Narrower stereo at low rates.
        let stereo_width_q14 = if equiv > 32_000 {
            16384
        } else if equiv < 16_000 {
            0
        } else {
            16384 - 2048 * (32_000 - equiv) / (equiv - 14_000)
        };
        if ch == 2 && (self.hybrid_stereo_width_q14 < 1 << 14 || stereo_width_q14 < 1 << 14) {
            let g1 = self.hybrid_stereo_width_q14 as f32 * (1.0 / 16384.0);
            let g2 = stereo_width_q14 as f32 * (1.0 / 16384.0);
            stereo_fade(&mut pcm_buf[..n * 2], g1, g2);
            self.hybrid_stereo_width_q14 = stereo_width_q14;
        }

        let mut enc = RangeEncoder::new(max_data_bytes - 1);
        let frame = CeltEncodeFrame {
            frame_size: n,
            start_band: 0,
            end_band: bandwidth.end_band(),
            stream_channels: self.stream_channels,
            bitrate: vbr.then_some(bitrate),
            vbr,
            constrained_vbr: self.config.constrained_vbr,
            silk_info: SilkInfo::default(),
        };
        let len = self.celt.encode(&pcm_buf[..n * ch], &frame, &mut enc)?;
        self.final_range = self.celt.final_range();
        self.first = false;
        let data = enc
            .done()
            .ok_or(Error::Corrupt("the encoder overflowed its packet"))?;
        debug_assert_eq!(data.len(), len);
        packet.push(celt_toc(
            frame_rate as usize,
            bandwidth,
            self.stream_channels == 2,
        ));
        packet.extend_from_slice(&data);
        Ok(())
    }

    /// The rate-dependent bandwidth, with hysteresis after the first frame.
    fn choose_bandwidth(&mut self, equiv: i32) -> Bw {
        // NB-MB, MB-WB, WB-SWB, SWB-FB: (threshold, hysteresis), for voice and for music;
        // the stereo tables are the same.
        const VOICE: [i32; 8] = [9000, 700, 9000, 700, 13500, 1000, 14000, 2000];
        const MUSIC: [i32; 8] = [9000, 700, 9000, 700, 11000, 1000, 12000, 2000];
        let mut thresholds = [0i32; 8];
        for i in 0..8 {
            thresholds[i] = MUSIC[i] + ((VOICE_EST * VOICE_EST * (VOICE[i] - MUSIC[i])) >> 14);
        }
        let mut bw = Bw::Full as usize;
        loop {
            let k = 2 * (bw - Bw::Medium as usize);
            let mut threshold = thresholds[k];
            let hysteresis = thresholds[k + 1];
            if !self.first {
                if self.auto_bandwidth.is_some_and(|a| a as usize >= bw) {
                    threshold -= hysteresis;
                } else {
                    threshold += hysteresis;
                }
            }
            if equiv >= threshold {
                break;
            }
            bw -= 1;
            if bw <= Bw::Narrow as usize {
                break;
            }
        }
        let mut bandwidth = Bw::from_index(bw);
        // Medium band only on request or in transitions.
        if bandwidth == Bw::Medium {
            bandwidth = Bw::Wide;
        }
        self.auto_bandwidth = Some(bandwidth);
        bandwidth
    }

    /// `dc_reject` at 3 Hz: a one-pole high-pass from `pcm` into `out`.
    fn dc_reject(&mut self, pcm: &[f32], out: &mut [f32]) {
        let coef = 6.3 * 3.0 / 48_000.0f32;
        let coef2 = 1.0 - coef;
        let ch = self.channels;
        for c in 0..ch {
            let mut m = self.hp_mem[2 * c];
            for (o, &x) in out
                .iter_mut()
                .skip(c)
                .step_by(ch)
                .zip(pcm.iter().skip(c).step_by(ch))
            {
                *o = x - m;
                m = coef * x + 1e-30 + coef2 * m;
            }
            self.hp_mem[2 * c] = m;
        }
    }
}

/// `stereo_fade`: narrows the stereo image from width `g1` to `g2` (1 is full), cross-faded
/// over the overlap with CELT's window.
fn stereo_fade(pcm: &mut [f32], g1: f32, g2: f32) {
    let g1 = 1.0 - g1;
    let g2 = 1.0 - g2;
    for (i, s) in pcm.as_chunks_mut::<2>().0.iter_mut().enumerate() {
        let g = if i < OVERLAP {
            let w = WINDOW[i] * WINDOW[i];
            w * g2 + (1.0 - w) * g1
        } else {
            g2
        };
        let diff = g * (0.5 * (s[0] - s[1]));
        s[0] -= diff;
        s[1] += diff;
    }
}

#[cfg(test)]
#[path = "encoder_tests.rs"]
pub(crate) mod tests;
