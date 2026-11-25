//! The Opus encoder: a port of libopus 1.5.2's `src/opus_encoder.c` (`opus_encode_native`,
//! `opus_encode_frame_native`) for `OPUS_APPLICATION_AUDIO` and `OPUS_APPLICATION_VOIP`,
//! with the tonality and speech/music analysis (`crate::analysis`, at the reference's
//! complexity 9 always on): the input filter (the 3 Hz DC filter, or for VoIP the high-pass
//! following SILK's pitch), the 4 ms delay that keeps SILK, hybrid and CELT aligned, the voice
//! estimate (from the analysis's music probability, or the hint), activity for SILK and DTX,
//! the detected bandwidth at low rates, the analysis handed to CELT, the rate-driven
//! decisions with their hysteresis (mono or stereo, SILK/hybrid or CELT, the bandwidth from
//! narrowband to fullband), SILK-only, hybrid (SILK then CELT from band 17 in one range
//! coder, the rate split between them, the high band's gain and the stereo width) and
//! CELT-only frames, the mode switches with their 5 ms redundant CELT frames and the prefills
//! they need, in-band FEC and DTX, CBR and (constrained) VBR, and packets of 2.5 to 60 ms (40
//! and 60 ms SILK-only frames whole, CELT and hybrid as 20 ms frames in one packet).
//!
//! Deviations from the reference, where its behaviour is accidental: a packet of several
//! frames prefills SILK once (the reference does it before every frame), and a stereo-to-mono
//! switch in such a packet downmixes in its last frame and goes mono in the next packet (the
//! reference sets `force_channels` for good).

use crate::analysis::{AnalysisInfo, TonalityAnalysis, maxabs};
use crate::celt::{
    CELT_DELAY, CeltEncodeFrame, CeltEncoder, OVERLAP, SilkInfo, WINDOW, exp2 as celt_exp2,
};
use crate::error::{Error, Result};
use crate::head::{ChannelMapping, OpusHead};
use crate::packet::{Bandwidth, Mode, Packet};
use crate::range_enc::RangeEncoder;
use crate::repacketizer::{assemble, pad};
use crate::silk::enc::{
    EncControl, Prefill, SilkEncoder, hp_cutoff_coefs, hp_min_cutoff_smth_q15, hp_smooth_cutoff,
};

/// Samples per channel of the encoder's delay line (`encoder_buffer`, 10 ms).
const ENCODER_BUFFER: usize = 480;
/// `delay_compensation` (4 ms: SILK's look-ahead, its resamplers and stereo prediction).
const DELAY_COMPENSATION: usize = 192;
/// The reference's default complexity.
const COMPLEXITY: i32 = 9;
/// The longest frame: a TOC byte and 1275 bytes.
const MAX_FRAME_PACKET: usize = 1276;
/// The room a packet is given: three frames of 1275 bytes and their framing.
const MAX_PACKET: usize = 3 * 1275 + 7;
/// `NB_SPEECH_FRAMES_BEFORE_DTX` and `MAX_CONSECUTIVE_DTX` (20 ms frames).
const NB_SPEECH_FRAMES_BEFORE_DTX: i32 = 10;
const MAX_CONSECUTIVE_DTX: i32 = 20;
/// The analysis's activity probability below which a frame may be inactive (`silk/define.h`).
const DTX_ACTIVITY_THRESHOLD: f32 = 0.1;
/// An inactive frame this much louder than the loudest active one so far (25 dB) is active.
const PSEUDO_SNR_THRESHOLD: f32 = 316.23;

/// The duration of each packet.
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
    /// 40 ms: one SILK frame, or two 20 ms CELT or hybrid frames.
    Ms40,
    /// 60 ms: one SILK frame, or three 20 ms CELT or hybrid frames.
    Ms60,
}

impl FrameDuration {
    /// Samples per channel at 48 kHz.
    pub fn samples(self) -> usize {
        match self {
            Self::Ms2_5 => 120,
            Self::Ms5 => 240,
            Self::Ms10 => 480,
            Self::Ms20 => 960,
            Self::Ms40 => 1920,
            Self::Ms60 => 2880,
        }
    }
}

/// What the input is, for the mode and bandwidth decisions (`OPUS_SET_SIGNAL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Signal {
    /// No hint: the tonality analysis tells speech from music, as in libopus.
    #[default]
    Auto,
    /// Speech: SILK and hybrid up to higher rates.
    Voice,
    /// Music: CELT from lower rates.
    Music,
}

/// What the encoder is tuned for (`OPUS_SET_APPLICATION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Application {
    /// Music and mixed content: a 3 Hz DC filter, CELT from lower rates.
    #[default]
    Audio,
    /// Speech: a high-pass that follows the voice's pitch, SILK up to higher rates.
    Voip,
}

/// What an [`Encoder`] makes: 48 kHz input, one or two channels.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncoderConfig {
    /// 1 or 2.
    pub channels: u8,
    /// Bits per second for the whole stream (clamped to 500 to 300000 per channel, as
    /// libopus does). With too few bits for a frame, a packet holds only its TOC byte (and in
    /// CBR its framing): the decoder conceals it, as libopus's "PLC frames".
    pub bitrate: u32,
    /// Variable bitrate (`OPUS_SET_VBR`); false is CBR, every packet the same size.
    pub vbr: bool,
    /// With `vbr`, keep each CELT packet within reach of the bitrate
    /// (`OPUS_SET_VBR_CONSTRAINT`). Unconstrained by default, as `opusenc`.
    pub constrained_vbr: bool,
    pub frame_duration: FrameDuration,
    /// The kind of input, for the mode decision.
    pub signal: Signal,
    pub application: Application,
    /// A mode to use whatever the rate (`OPUS_SET_FORCE_MODE`); `None` decides by the rate.
    /// As in libopus, SILK above wideband becomes hybrid, hybrid at or below wideband becomes
    /// SILK, and frames under 10 ms are always CELT.
    pub mode: Option<Mode>,
    /// A bandwidth to use whatever the rate (`OPUS_SET_BANDWIDTH`); `None` decides by the
    /// rate. CELT codes medium-band as wideband.
    pub bandwidth: Option<Bandwidth>,
    /// SILK's in-band forward error correction (`OPUS_SET_INBAND_FEC`): each SILK or hybrid
    /// packet also carries a coarse copy of the previous one, when the rate allows.
    pub inband_fec: bool,
    /// The packet loss to expect, 0 to 100 (`OPUS_SET_PACKET_LOSS_PERC`): FEC needs it above
    /// 0, and it makes SILK code more robustly.
    pub packet_loss_percent: u8,
    /// Discontinuous transmission (`OPUS_SET_DTX`): inactive frames (by the analysis's
    /// activity, or digital silence) are sent as TOC-only packets, one in every 400 ms.
    pub dtx: bool,
    /// The depth of the input in bits, 8 to 24 (`OPUS_SET_LSB_DEPTH`; anything else is an
    /// error, as libopus refuses it): what counts as digital silence and the noise floor
    /// the bandwidth detection and CELT's band allocation assume. 16 for 16-bit sources,
    /// as `opusenc` sets it.
    pub lsb_depth: u8,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            channels: 2,
            bitrate: 128_000,
            vbr: true,
            constrained_vbr: false,
            frame_duration: FrameDuration::Ms20,
            signal: Signal::Auto,
            application: Application::Audio,
            mode: None,
            bandwidth: None,
            inband_fec: false,
            packet_loss_percent: 0,
            dtx: false,
            lsb_depth: 24,
        }
    }
}

/// CELT's end band (`CELT_SET_END_BAND`).
fn end_band(bw: Bandwidth) -> usize {
    bw.celt_end_band()
}

/// `gen_toc`.
fn gen_toc(mode: Mode, frame_rate: usize, bandwidth: Bandwidth, stream_channels: usize) -> u8 {
    let mut period = 0u8;
    let mut rate = frame_rate;
    while rate < 400 {
        rate <<= 1;
        period += 1;
    }
    let bw = bandwidth as u8;
    let toc = match mode {
        Mode::Silk => (bw << 5) | (period.wrapping_sub(2) << 3),
        Mode::Celt => 0x80 | (bw.saturating_sub(Bandwidth::Medium as u8) << 5) | (period << 3),
        Mode::Hybrid => {
            0x60 | (bw.wrapping_sub(Bandwidth::SuperWide as u8) << 4)
                | (period.wrapping_sub(2) << 3)
        }
    };
    toc | (u8::from(stream_channels == 2) << 2)
}

/// `compute_equiv_rate` at complexity 9: the 20 ms VBR rate of the same quality. `mode` is
/// `None` before the mode is known.
fn equiv_rate(
    bitrate: i32,
    channels: i32,
    frame_rate: i32,
    vbr: bool,
    mode: Option<Mode>,
    loss: i32,
) -> i32 {
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
    equiv = equiv * (90 + COMPLEXITY) / 100;
    match mode {
        Some(Mode::Silk | Mode::Hybrid) => equiv -= equiv * loss / (6 * loss + 10),
        Some(Mode::Celt) => {}
        // Half SILK's loss.
        None => equiv -= equiv * loss / (12 * loss + 20),
    }
    equiv
}

/// `compute_silk_rate_for_hybrid`: SILK's share of a hybrid frame's `rate`.
fn silk_rate_for_hybrid(
    rate: i32,
    bandwidth: Bandwidth,
    frame20ms: bool,
    vbr: bool,
    fec: bool,
    channels: i32,
) -> i32 {
    // Total, then SILK's share without FEC (10 ms, 20 ms) and with it.
    const RATES: [[i32; 5]; 7] = [
        [0, 0, 0, 0, 0],
        [12000, 10000, 10000, 11000, 11000],
        [16000, 13500, 13500, 15000, 15000],
        [20000, 16000, 16000, 18000, 18000],
        [24000, 18000, 18000, 21000, 21000],
        [32000, 22000, 22000, 28000, 28000],
        [64000, 38000, 38000, 50000, 50000],
    ];
    let rate = rate / channels;
    let entry = 1 + usize::from(frame20ms) + 2 * usize::from(fec);
    let i = (1..RATES.len())
        .find(|&i| RATES[i][0] > rate)
        .unwrap_or(RATES.len());
    let mut silk = if i == RATES.len() {
        // Half of what is left over goes to SILK.
        RATES[i - 1][entry] + (rate - RATES[i - 1][0]) / 2
    } else {
        let (lo, hi) = (RATES[i - 1][entry], RATES[i][entry]);
        let (x0, x1) = (RATES[i - 1][0], RATES[i][0]);
        (lo * (x1 - rate) + hi * (rate - x0)) / (x1 - x0)
    };
    if !vbr {
        // A small boost for CBR.
        silk += 100;
    }
    if bandwidth == Bandwidth::SuperWide {
        silk += 300;
    }
    silk *= channels;
    // Calibrated for stereo at 32 kb/s.
    if channels == 2 && rate >= 12000 {
        silk -= 1000;
    }
    silk
}

/// `decide_fec`: whether SILK codes LBRR at `rate`, perhaps narrowing `bandwidth` for it.
fn decide_fec(
    fec: bool,
    loss: i32,
    last_fec: bool,
    mode: Mode,
    bandwidth: &mut Bandwidth,
    rate: i32,
) -> bool {
    const THRESHOLDS: [i32; 10] = [
        12000, 1000, 14000, 1000, 16000, 1000, 20000, 1000, 22000, 1000,
    ];
    if !fec || loss == 0 || mode == Mode::Celt {
        return false;
    }
    let orig = *bandwidth;
    loop {
        let k = 2 * (*bandwidth as usize);
        let mut threshold = THRESHOLDS[k];
        let hysteresis = THRESHOLDS[k + 1];
        if last_fec {
            threshold -= hysteresis;
        } else {
            threshold += hysteresis;
        }
        // silk_SMULWB(threshold * (125 - min(loss, 25)), 0.01 in Q16).
        let scaled = threshold * (125 - loss.min(25));
        threshold = ((i64::from(scaled) * i64::from(655i16)) >> 16) as i32;
        if rate > threshold {
            return true;
        } else if loss <= 5 {
            return false;
        } else if *bandwidth > Bandwidth::Narrow {
            *bandwidth = Bandwidth::from_index(*bandwidth as usize - 1);
        } else {
            break;
        }
    }
    *bandwidth = orig;
    false
}

impl Bandwidth {
    fn from_index(i: usize) -> Self {
        [
            Self::Narrow,
            Self::Medium,
            Self::Wide,
            Self::SuperWide,
            Self::Full,
        ][i.min(4)]
    }
}

/// `compute_redundancy_bytes`: the size of a 5 ms redundant CELT frame, or 0 when too few
/// bits would make one worth less than the decoder's concealment.
fn compute_redundancy_bytes(
    max_data_bytes: i32,
    bitrate: i32,
    frame_rate: i32,
    channels: i32,
) -> i32 {
    let base_bits = 40 * channels + 20;
    // The equivalent rate for 5 ms frames, and half as much again: it is short.
    let rate = 3 * (bitrate + base_bits * (200 - frame_rate)) / 2;
    let bytes = rate / 1600;
    let available = max_data_bytes * 8 - 2 * base_bits;
    let cap = (available * 240 / (240 + 48000 / frame_rate) + base_bits) / 8;
    let bytes = bytes.min(cap);
    if bytes > 4 + 8 * channels {
        bytes.min(257)
    } else {
        0
    }
}

/// `StereoWidthState`.
#[derive(Debug, Clone, Copy, Default)]
struct StereoWidth {
    xx: f32,
    xy: f32,
    yy: f32,
    smoothed_width: f32,
    max_follower: f32,
}

impl StereoWidth {
    /// `compute_stereo_width`: how wide the stereo image is, 0 to 1, smoothed over a second.
    fn update(&mut self, pcm: &[f32], frame_size: usize) -> f32 {
        let frame_rate = (48_000 / frame_size) as f32;
        let short_alpha = 1.0 - 25.0 / frame_rate.max(50.0);
        let (mut xx, mut xy, mut yy) = (0.0f32, 0.0f32, 0.0f32);
        let mut i = 0;
        while i + 3 < frame_size {
            let (mut pxx, mut pxy, mut pyy) = (0.0f32, 0.0f32, 0.0f32);
            for k in 0..4 {
                let x = pcm[2 * (i + k)];
                let y = pcm[2 * (i + k) + 1];
                pxx += x * x;
                pxy += x * y;
                pyy += y * y;
            }
            xx += pxx;
            xy += pxy;
            yy += pyy;
            i += 4;
        }
        let finite = |v: f32| v.partial_cmp(&1e9) == Some(std::cmp::Ordering::Less);
        if !finite(xx) || !finite(yy) {
            (xx, xy, yy) = (0.0, 0.0, 0.0);
        }
        self.xx += short_alpha * (xx - self.xx);
        self.xy += short_alpha * (xy - self.xy);
        self.yy += short_alpha * (yy - self.yy);
        self.xx = self.xx.max(0.0);
        self.xy = self.xy.max(0.0);
        self.yy = self.yy.max(0.0);
        if self.xx.max(self.yy) > 8e-4 {
            let sqrt_xx = self.xx.sqrt();
            let sqrt_yy = self.yy.sqrt();
            let qrrt_xx = sqrt_xx.sqrt();
            let qrrt_yy = sqrt_yy.sqrt();
            // The inter-channel correlation and about the loudness difference.
            self.xy = self.xy.min(sqrt_xx * sqrt_yy);
            let corr = self.xy / (1e-15 + sqrt_xx * sqrt_yy);
            let ldiff = (qrrt_xx - qrrt_yy).abs() / (1e-15 + qrrt_xx + qrrt_yy);
            let width = (1.0 - corr * corr).sqrt() * ldiff;
            // Smoothed over a second, with a peak follower.
            self.smoothed_width += (width - self.smoothed_width) / frame_rate;
            self.max_follower = (self.max_follower - 0.02 / frame_rate).max(self.smoothed_width);
        }
        (20.0 * self.max_follower).min(1.0)
    }
}

/// `compute_frame_energy`: the mean square of the frame, summed in order.
fn frame_energy(pcm: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    for &v in pcm {
        sum += v * v;
    }
    sum / pcm.len().max(1) as f32
}

/// `FLOAT2INT16`: full scale 1.0 to 16 bits, rounded to nearest even and saturated.
fn float2int16(x: f32) -> i16 {
    (x * 32768.0).clamp(-32768.0, 32767.0).round_ties_even() as i16
}

/// `gain_fade`: from gain `g1` to `g2` over the overlap (CELT's window squared), then `g2`.
fn gain_fade(pcm: &mut [f32], g1: f32, g2: f32, frame_size: usize, channels: usize) {
    for i in 0..frame_size {
        let g = if i < OVERLAP {
            let w = WINDOW[i] * WINDOW[i];
            w * g2 + (1.0 - w) * g1
        } else {
            g2
        };
        for v in &mut pcm[i * channels..(i + 1) * channels] {
            *v *= g;
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

/// What `opus_encode_native` decided for one packet, handed to each of its frames.
#[derive(Debug, Clone, Copy)]
struct FramePlan {
    redundancy: bool,
    celt_to_silk: bool,
    prefill: bool,
    equiv_rate: i32,
    to_celt: bool,
    is_silence: bool,
    /// What the tonality analysis says of the frame.
    analysis: AnalysisInfo,
}

/// An Opus encoder: 48 kHz PCM in, one standard Opus packet per call out. A port of libopus
/// 1.5.2's float encoder at complexity 9, its tonality analysis included (its SILK layer the
/// fixed-point build's); see the crate's `COPYING`.
#[derive(Debug, Clone)]
pub struct Encoder {
    config: EncoderConfig,
    channels: usize,
    frame_size: usize,
    /// The user's bitrate, clamped as `OPUS_SET_BITRATE` does.
    user_bitrate: i32,
    /// `OPUS_SET_FORCE_CHANNELS` (the multistream encoder's coupled streams).
    force_channels: Option<usize>,
    /// `OPUS_SET_LFE`: CELT-only narrowband, CELT's LFE mode.
    lfe: bool,
    celt: CeltEncoder,
    silk: SilkEncoder,
    silk_mode: EncControl,
    /// The tonality and speech/music analysis (`run_analysis`), at the reference's
    /// complexity 9 always on.
    analysis: Box<TonalityAnalysis>,
    /// The analysis's percentage of voice, -1 when there is none (a reset keeps it, as in
    /// libopus).
    voice_ratio: i32,
    /// What the last frame was told by the analysis, and the voice estimate (for tests).
    pub(crate) last_analysis: AnalysisInfo,
    pub(crate) last_voice_est: i32,
    // From here on, what OPUS_RESET_STATE clears.
    bitrate: i32,
    stream_channels: usize,
    hybrid_stereo_width_q14: i32,
    variable_hp_smth2_q15: i32,
    prev_hb_gain: f32,
    /// The input filter's memory: the DC filter's in entries 0 and 2, the high-pass's in
    /// 0-1 and 2-3.
    hp_mem: [f32; 4],
    mode: Mode,
    prev_mode: Option<Mode>,
    prev_channels: usize,
    bandwidth: Bandwidth,
    auto_bandwidth: Option<Bandwidth>,
    silk_bw_switch: bool,
    first: bool,
    width_mem: StereoWidth,
    /// The last [`ENCODER_BUFFER`] samples per channel, interleaved (`delay_buffer`).
    delay_buffer: [f32; ENCODER_BUFFER * 2],
    nb_no_activity_ms_q1: i32,
    /// The surround masks of this stream's channels (`OPUS_SET_ENERGY_MASK`, from the
    /// multistream encoder; a reset forgets them).
    energy_masking: Option<[f32; 42]>,
    /// The energy of the loudest active frame, decaying.
    peak_signal_energy: f32,
    nonfinal_frame: bool,
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
        if config.packet_loss_percent > 100 {
            return Err(Error::BadArgument("a packet loss above 100%"));
        }
        if !(8..=24).contains(&config.lsb_depth) {
            return Err(Error::BadArgument("an input depth outside 8 to 24 bits"));
        }
        let channels = usize::from(config.channels);
        let user_bitrate = i64::from(config.bitrate).clamp(500, 300_000 * channels as i64) as i32;
        let mut enc = Self {
            config: config.clone(),
            channels,
            frame_size: config.frame_duration.samples(),
            user_bitrate,
            force_channels: None,
            lfe: false,
            celt: CeltEncoder::new(channels),
            silk: SilkEncoder::new(),
            silk_mode: EncControl::default(),
            analysis: Box::default(),
            voice_ratio: -1,
            last_analysis: AnalysisInfo::default(),
            last_voice_est: 0,
            bitrate: user_bitrate,
            stream_channels: channels,
            hybrid_stereo_width_q14: 1 << 14,
            variable_hp_smth2_q15: 0,
            prev_hb_gain: 1.0,
            hp_mem: [0.0; 4],
            mode: Mode::Hybrid,
            prev_mode: None,
            prev_channels: 0,
            bandwidth: Bandwidth::Full,
            auto_bandwidth: None,
            silk_bw_switch: false,
            first: true,
            width_mem: StereoWidth::default(),
            delay_buffer: [0.0; ENCODER_BUFFER * 2],
            nb_no_activity_ms_q1: 0,
            energy_masking: None,
            peak_signal_energy: 0.0,
            nonfinal_frame: false,
            final_range: 0,
        };
        enc.reset();
        Ok(enc)
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
    /// decoder of the packet reports (of its last frame). 0 after a packet with no frame data.
    pub fn final_range(&self) -> u32 {
        self.final_range
    }

    /// Forgets the stream, as if new (`OPUS_RESET_STATE`, which also starts SILK's controls
    /// afresh here).
    pub fn reset(&mut self) {
        self.analysis.reset();
        self.celt.reset();
        self.silk = SilkEncoder::new();
        self.silk_mode = EncControl {
            n_channels_api: self.channels,
            n_channels_internal: self.channels,
            packet_loss_percentage: i32::from(self.config.packet_loss_percent),
            ..EncControl::default()
        };
        self.bitrate = self.user_bitrate;
        self.stream_channels = self.channels;
        self.hybrid_stereo_width_q14 = 1 << 14;
        self.variable_hp_smth2_q15 = hp_min_cutoff_smth_q15();
        self.prev_hb_gain = 1.0;
        self.hp_mem = [0.0; 4];
        self.mode = Mode::Hybrid;
        self.prev_mode = None;
        self.prev_channels = 0;
        self.bandwidth = Bandwidth::Full;
        self.auto_bandwidth = None;
        self.silk_bw_switch = false;
        self.first = true;
        self.width_mem = StereoWidth::default();
        self.delay_buffer = [0.0; ENCODER_BUFFER * 2];
        self.nb_no_activity_ms_q1 = 0;
        self.energy_masking = None;
        self.peak_signal_energy = 0.0;
        self.nonfinal_frame = false;
        self.final_range = 0;
    }

    /// The `OpusHead` for a stream of these packets (family 0, pre-skip from the lookahead,
    /// no gain), recording `input_sample_rate` (informational: what the audio was before
    /// resampling to 48 kHz, 0 if unknown). Its [`OpusHead::to_bytes`] is the first packet of
    /// an Ogg Opus stream.
    pub fn head(&self, input_sample_rate: u32) -> OpusHead {
        let channels = self.config.channels;
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

    /// `OPUS_SET_LFE` for the multistream encoder: CELT-only narrowband in CELT's LFE mode.
    pub(crate) fn set_lfe(&mut self, lfe: bool) {
        self.lfe = lfe;
        self.celt.set_lfe(lfe);
    }

    /// `OPUS_SET_ENERGY_MASK` for the multistream encoder: the surround masks of this
    /// stream's channels, 21 bands each, for the next packet.
    pub(crate) fn set_energy_mask(&mut self, mask: Option<[f32; 42]>) {
        self.energy_masking = mask;
        self.celt.set_energy_mask(mask.as_ref());
    }

    /// `OPUS_SET_FORCE_CHANNELS` for the multistream encoder.
    pub(crate) fn set_force_channels(&mut self, channels: Option<usize>) {
        self.force_channels = channels;
    }

    /// Changes the bitrate from the next packet on (`OPUS_SET_BITRATE`), clamped as in
    /// [`EncoderConfig::bitrate`]; 0 is refused.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<()> {
        if bitrate == 0 {
            return Err(Error::BadArgument("a bitrate of 0"));
        }
        self.config.bitrate = bitrate;
        self.user_bitrate = i64::from(bitrate).clamp(500, 300_000 * self.channels as i64) as i32;
        Ok(())
    }

    /// `OPUS_SET_FORCE_MODE` and `OPUS_SET_BANDWIDTH` for the multistream encoder.
    pub(crate) fn set_mode_and_bandwidth(&mut self, mode: Option<Mode>, bw: Option<Bandwidth>) {
        self.config.mode = mode;
        self.config.bandwidth = bw;
    }

    /// Encodes exactly [`frame_size`](Self::frame_size) samples per channel, interleaved,
    /// full scale 1.0, into `packet` (cleared first). A frame holding a NaN, an infinity or
    /// absurdly large samples (energy of 1e9 and up) is coded as silence, as libopus does.
    pub fn encode(&mut self, pcm: &[f32], packet: &mut Vec<u8>) -> Result<()> {
        if pcm.len() != self.frame_size * self.channels {
            return Err(Error::BadArgument(
                "an Opus encoder takes one frame of interleaved samples",
            ));
        }
        packet.clear();
        self.encode_native(pcm, MAX_PACKET, packet)
    }

    /// `opus_encode_native` into at most `out_data_bytes`: the decisions for the packet, then
    /// its frame or frames.
    pub(crate) fn encode_native(
        &mut self,
        pcm: &[f32],
        out_data_bytes: usize,
        packet: &mut Vec<u8>,
    ) -> Result<()> {
        let ch = self.channels;
        let frame_size = self.frame_size;
        let vbr = self.config.vbr;
        let loss = i32::from(self.config.packet_loss_percent);
        let mut max_data_bytes = out_data_bytes.min(MAX_FRAME_PACKET) as i32;
        self.final_range = 0;

        let is_silence = maxabs(pcm) <= 1.0 / (1 << self.config.lsb_depth) as f32;
        let analysis_read_bak = self.analysis.read_position();
        let analysis_info = self
            .analysis
            .run(pcm, ch, frame_size, self.config.lsb_depth);
        // The loudest active frame so far, for the activity of noisy ones.
        if !is_silence && analysis_info.activity_probability > DTX_ACTIVITY_THRESHOLD {
            self.peak_signal_energy = (0.999 * self.peak_signal_energy).max(frame_energy(pcm));
        }
        // The voice ratio of the last frame that was not silent.
        if !is_silence {
            self.voice_ratio = -1;
        }
        let mut detected_bandwidth = None;
        if analysis_info.valid {
            if self.config.signal == Signal::Auto {
                let prob = match self.prev_mode {
                    None => analysis_info.music_prob,
                    Some(Mode::Celt) => analysis_info.music_prob_max,
                    Some(_) => analysis_info.music_prob_min,
                };
                self.voice_ratio = libm::floor(0.5 + 100.0 * f64::from(1.0 - prob)) as i32;
            }
            detected_bandwidth = Some(match analysis_info.bandwidth {
                ..=12 => Bandwidth::Narrow,
                13..=14 => Bandwidth::Medium,
                15..=16 => Bandwidth::Wide,
                17..=18 => Bandwidth::SuperWide,
                _ => Bandwidth::Full,
            });
        }
        let stereo_width = if ch == 2 && self.force_channels != Some(1) {
            self.width_mem.update(pcm, frame_size)
        } else {
            0.0
        };
        self.bitrate = self.user_bitrate;
        let mut frame_rate = (48_000 / frame_size) as i32;
        let mut cbr_bytes = -1;
        if !vbr {
            // Times 12 so that the division is exact.
            let frame_rate12 = 12 * 48_000 / frame_size as i32;
            cbr_bytes =
                ((12 * self.bitrate / 8 + frame_rate12 / 2) / frame_rate12).min(max_data_bytes);
            self.bitrate = cbr_bytes * frame_rate12 * 8 / 12;
            max_data_bytes = cbr_bytes.max(1);
        }
        if max_data_bytes < 3
            || self.bitrate < 3 * frame_rate * 8
            || (frame_rate < 50 && (max_data_bytes * frame_rate < 300 || self.bitrate < 2400))
        {
            // Too few bits to code anything useful: a packet the decoder conceals.
            let mut tocmode = self.mode;
            let mut bw = self.bandwidth;
            let mut code = 0u8;
            let mut multiframes = 0u8;
            if frame_rate > 100 {
                tocmode = Mode::Celt;
            }
            // 40 ms as two 20 ms frames in CELT or hybrid mode.
            if frame_rate == 25 && tocmode != Mode::Silk {
                frame_rate = 50;
                code = 1;
            }
            if frame_rate <= 16 {
                if out_data_bytes == 1 || (tocmode == Mode::Silk && frame_rate != 10) {
                    tocmode = Mode::Silk;
                    code = u8::from(frame_rate <= 12);
                    frame_rate = if frame_rate == 12 { 25 } else { 16 };
                } else {
                    multiframes = (50 / frame_rate) as u8;
                    frame_rate = 50;
                    code = 3;
                }
            }
            if tocmode == Mode::Silk && bw > Bandwidth::Wide {
                bw = Bandwidth::Wide;
            } else if tocmode == Mode::Celt && bw == Bandwidth::Medium {
                bw = Bandwidth::Narrow;
            } else if tocmode == Mode::Hybrid && bw <= Bandwidth::SuperWide {
                bw = Bandwidth::SuperWide;
            }
            packet.push(gen_toc(tocmode, frame_rate as usize, bw, self.stream_channels) | code);
            if code == 3 {
                packet.push(multiframes);
            }
            if !vbr {
                pad(packet, (max_data_bytes as usize).max(packet.len()))?;
            }
            return Ok(());
        }
        let max_rate = frame_rate * max_data_bytes * 8;

        // The equivalent 20 ms rate for the mode, channel and bandwidth decisions.
        let mut equiv = equiv_rate(self.bitrate, ch as i32, frame_rate, vbr, None, loss);

        let voice_est: i32 = match (self.config.signal, self.config.application) {
            (Signal::Voice, _) => 127,
            (Signal::Music, _) => 0,
            // At most 90% sure of speech for audio.
            (Signal::Auto, Application::Audio) if self.voice_ratio >= 0 => {
                ((self.voice_ratio * 327) >> 8).min(115)
            }
            (Signal::Auto, Application::Voip) if self.voice_ratio >= 0 => {
                (self.voice_ratio * 327) >> 8
            }
            (Signal::Auto, Application::Voip) => 115,
            (Signal::Auto, Application::Audio) => 48,
        };
        self.last_voice_est = voice_est;

        if let (Some(forced), 2) = (self.force_channels, ch) {
            self.stream_channels = forced;
        } else if ch == 2 {
            // Stereo above about 17 kb/s, with hysteresis.
            let mut threshold = 17_000 + ((voice_est * voice_est * (19_000 - 17_000)) >> 14);
            if self.stream_channels == 2 {
                threshold -= 1000;
            } else {
                threshold += 1000;
            }
            self.stream_channels = if equiv > threshold { 2 } else { 1 };
        } else {
            self.stream_channels = ch;
        }
        equiv = equiv_rate(
            self.bitrate,
            self.stream_channels as i32,
            frame_rate,
            vbr,
            None,
            loss,
        );

        // SILK's DTX where the general one (the analysis's activity, or digital silence) does
        // not apply.
        self.silk_mode.use_dtx = self.config.dtx && !(analysis_info.valid || is_silence);

        if let Some(forced) = self.config.mode {
            self.mode = forced;
        } else {
            // The SILK/CELT threshold, between voice's and music's by the stereo width and the
            // voice estimate. (libopus takes the stereo music threshold for both widths.)
            let mode_voice = ((1.0 - stereo_width) * 64_000.0 + stereo_width * 44_000.0) as i32;
            let mode_music = ((1.0 - stereo_width) * 10_000.0 + stereo_width * 10_000.0) as i32;
            let mut threshold =
                mode_music + ((voice_est * voice_est * (mode_voice - mode_music)) >> 14);
            if self.config.application == Application::Voip {
                // SILK has features VoIP wants.
                threshold += 8000;
            }
            match self.prev_mode {
                Some(Mode::Celt) => threshold -= 4000,
                Some(_) => threshold += 4000,
                None => {}
            }
            self.mode = if equiv >= threshold {
                Mode::Celt
            } else {
                Mode::Silk
            };
            // FEC needs SILK when enough packets get lost.
            if self.config.inband_fec && loss > (128 - voice_est) >> 4 {
                self.mode = Mode::Silk;
            }
            // Speech with DTX uses SILK's.
            if self.silk_mode.use_dtx && voice_est > 100 {
                self.mode = Mode::Silk;
            }
            // Under 6 kb/s of room: CELT.
            let floor = if frame_rate > 50 { 9000 } else { 6000 };
            if max_data_bytes < floor * frame_size as i32 / (48_000 * 8) {
                self.mode = Mode::Celt;
            }
        }
        // Under 10 ms only CELT will do.
        if self.mode != Mode::Celt && frame_size < 480 {
            self.mode = Mode::Celt;
        }
        if self.lfe {
            self.mode = Mode::Celt;
        }

        let mut redundancy = false;
        let mut celt_to_silk = false;
        let mut to_celt = false;
        if let Some(prev) = self.prev_mode
            && ((self.mode != Mode::Celt && prev == Mode::Celt)
                || (self.mode == Mode::Celt && prev != Mode::Celt))
        {
            redundancy = true;
            celt_to_silk = self.mode != Mode::Celt;
            if !celt_to_silk {
                // SILK or hybrid one more frame, ending in a redundant CELT frame.
                if frame_size >= 480 {
                    self.mode = prev;
                    to_celt = true;
                } else {
                    redundancy = false;
                }
            }
        }

        // Stereo to mono after two frames, so that SILK downmixes smoothly.
        if self.stream_channels == 1
            && self.prev_channels == 2
            && !self.silk_mode.to_mono
            && self.mode != Mode::Celt
            && self.prev_mode != Some(Mode::Celt)
        {
            self.silk_mode.to_mono = true;
            self.stream_channels = 2;
        } else {
            self.silk_mode.to_mono = false;
        }

        equiv = equiv_rate(
            self.bitrate,
            self.stream_channels as i32,
            frame_rate,
            vbr,
            Some(self.mode),
            loss,
        );

        let mut prefill = false;
        if self.mode != Mode::Celt && self.prev_mode == Some(Mode::Celt) {
            self.silk = SilkEncoder::new();
            prefill = true;
        }

        if self.mode == Mode::Celt || self.first || self.silk_mode.allow_bandwidth_switch {
            self.choose_bandwidth(equiv, voice_est);
        }
        if let Some(bw) = self.config.bandwidth {
            self.bandwidth = bw;
        }
        // No hybrid at unsafe CBR or maximum rates.
        if self.mode != Mode::Celt && max_rate < 15_000 {
            self.bandwidth = self.bandwidth.min(Bandwidth::Wide);
        }
        // No bits above the bandwidth the analysis found, conservatively: never below
        // wideband in SILK or hybrid (whose switches need redundancy), and only at low rates.
        if let (Some(detected), None) = (detected_bandwidth, self.config.bandwidth) {
            let sc = self.stream_channels as i32;
            let celt = self.mode == Mode::Celt;
            let min_detected = if equiv <= 18_000 * sc && celt {
                Bandwidth::Narrow
            } else if equiv <= 24_000 * sc && celt {
                Bandwidth::Medium
            } else if equiv <= 30_000 * sc {
                Bandwidth::Wide
            } else if equiv <= 44_000 * sc {
                Bandwidth::SuperWide
            } else {
                Bandwidth::Full
            };
            self.bandwidth = self.bandwidth.min(detected.max(min_detected));
        }
        let mut bandwidth = self.bandwidth;
        self.silk_mode.lbrr_coded = decide_fec(
            self.config.inband_fec,
            loss,
            self.silk_mode.lbrr_coded,
            self.mode,
            &mut bandwidth,
            equiv,
        );
        self.bandwidth = bandwidth;
        self.celt.set_lsb_depth(self.config.lsb_depth);
        // CELT has no medium band.
        if self.mode == Mode::Celt && self.bandwidth == Bandwidth::Medium {
            self.bandwidth = Bandwidth::Wide;
        }
        if self.lfe {
            self.bandwidth = Bandwidth::Narrow;
        }
        // SILK above wideband is hybrid, hybrid at or below it SILK; never to or from CELT here.
        if self.mode == Mode::Silk && self.bandwidth > Bandwidth::Wide {
            self.mode = Mode::Hybrid;
        }
        if self.mode == Mode::Hybrid && self.bandwidth <= Bandwidth::Wide {
            self.mode = Mode::Silk;
        }

        let plan = FramePlan {
            redundancy,
            celt_to_silk,
            prefill,
            equiv_rate: equiv,
            to_celt,
            is_silence,
            analysis: analysis_info,
        };
        if frame_size > 960 && self.mode != Mode::Silk {
            // CELT and hybrid frames are at most 20 ms: several in one packet.
            let nb_frames = frame_size / 960;
            let max_header_bytes = if nb_frames == 2 {
                3
            } else {
                2 + (nb_frames - 1) * 2
            };
            let repacketize_len = if vbr {
                out_data_bytes
            } else {
                (cbr_bytes as usize).min(out_data_bytes)
            };
            let max_len_sum = (nb_frames + repacketize_len).saturating_sub(max_header_bytes);
            let to_mono = self.silk_mode.to_mono;
            if !to_mono {
                self.prev_channels = self.stream_channels;
            }
            // Each frame reads its own analysis from the start of the packet.
            self.analysis.set_read_position(analysis_read_bak);
            let mut frames: Vec<Vec<u8>> = Vec::with_capacity(nb_frames);
            let mut tot_size = 0;
            let mut dtx_count = 0;
            for i in 0..nb_frames {
                let last = i == nb_frames - 1;
                // The downmix runs in the packet's last frame, mono from the next packet.
                self.silk_mode.to_mono = to_mono && last;
                self.nonfinal_frame = !last;
                let frame_to_celt = to_celt && last;
                let frame_redundancy = redundancy && (frame_to_celt || (!to_celt && i == 0));
                let mut curr_max = (3 * self.bitrate / (3 * 8 * 48_000 / 960)).max(0) as usize;
                curr_max = curr_max.min(max_len_sum / nb_frames);
                curr_max = curr_max.min(max_len_sum - tot_size);
                let mut frame = vec![];
                let frame_analysis = self.analysis.get_info(960);
                self.encode_frame_native(
                    &pcm[i * ch * 960..(i + 1) * ch * 960],
                    960,
                    curr_max as i32,
                    &FramePlan {
                        redundancy: frame_redundancy,
                        prefill: prefill && i == 0,
                        to_celt: frame_to_celt,
                        analysis: frame_analysis,
                        ..plan
                    },
                    &mut frame,
                )?;
                if frame.len() == 1 {
                    dtx_count += 1;
                }
                tot_size += frame.len();
                frames.push(frame);
            }
            self.silk_mode.to_mono = to_mono;
            let toc = frames[0][0];
            let mut parts: Vec<&[u8]> = Vec::with_capacity(nb_frames);
            for f in &frames {
                let p = Packet::parse(f)?;
                parts.extend_from_slice(p.frames());
            }
            *packet = assemble(
                toc,
                &parts,
                repacketize_len,
                false,
                !vbr && dtx_count != nb_frames,
            )?;
            return Ok(());
        }
        self.encode_frame_native(pcm, frame_size, max_data_bytes, &plan, packet)
    }

    /// The rate-dependent bandwidth, with hysteresis after the first frame.
    fn choose_bandwidth(&mut self, equiv: i32, voice_est: i32) {
        // NB-MB, MB-WB, WB-SWB, SWB-FB: (threshold, hysteresis), for voice and for music;
        // the stereo tables are the same.
        const VOICE: [i32; 8] = [9000, 700, 9000, 700, 13500, 1000, 14000, 2000];
        const MUSIC: [i32; 8] = [9000, 700, 9000, 700, 11000, 1000, 12000, 2000];
        let mut thresholds = [0i32; 8];
        for i in 0..8 {
            thresholds[i] = MUSIC[i] + ((voice_est * voice_est * (VOICE[i] - MUSIC[i])) >> 14);
        }
        let mut bw = Bandwidth::Full as usize;
        loop {
            let k = 2 * (bw - Bandwidth::Medium as usize);
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
            if bw <= Bandwidth::Narrow as usize {
                break;
            }
        }
        let mut bandwidth = Bandwidth::from_index(bw);
        // Medium band only on request or in transitions.
        if bandwidth == Bandwidth::Medium {
            bandwidth = Bandwidth::Wide;
        }
        self.bandwidth = bandwidth;
        self.auto_bandwidth = Some(bandwidth);
        // Not above wideband until SILK has switched to it and turned its low-pass off.
        if !self.first
            && self.mode != Mode::Celt
            && !self.silk_mode.in_wb_mode_without_variable_lp
            && self.bandwidth > Bandwidth::Wide
        {
            self.bandwidth = Bandwidth::Wide;
        }
    }

    /// `opus_encode_frame_native`: one frame of `frame_size` samples into `out` (a whole
    /// packet of at most `max_data_bytes`).
    fn encode_frame_native(
        &mut self,
        pcm: &[f32],
        frame_size: usize,
        max_data_bytes: i32,
        plan: &FramePlan,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let ch = self.channels;
        let vbr = self.config.vbr;
        self.final_range = 0;
        let mut curr_bandwidth = self.bandwidth;
        let total_buffer = DELAY_COMPENSATION;
        let frame_rate = (48_000 / frame_size) as i32;
        let max_data_bytes = max_data_bytes.max(1);
        // Activity from the analysis (a noisy frame much louder than the loudest active one
        // is active too); without it SILK decides.
        let activity = if plan.is_silence {
            Some(false)
        } else if plan.analysis.valid {
            let active = plan.analysis.activity_probability >= DTX_ACTIVITY_THRESHOLD;
            Some(active || self.peak_signal_energy < PSEUDO_SNR_THRESHOLD * frame_energy(pcm))
        } else {
            None
        };
        self.last_analysis = plan.analysis;
        let mut redundancy = plan.redundancy;
        let mut celt_to_silk = plan.celt_to_silk;
        let mut prefill = plan.prefill.then_some(Prefill::Reset);

        // The first frame at a new SILK bandwidth.
        if self.silk_bw_switch {
            redundancy = true;
            celt_to_silk = true;
            self.silk_bw_switch = false;
            prefill = Some(Prefill::KeepLowPass);
        }
        if self.mode == Mode::Celt {
            redundancy = false;
        }
        let mut redundancy_bytes = 0;
        if redundancy {
            redundancy_bytes = compute_redundancy_bytes(
                max_data_bytes,
                self.bitrate,
                frame_rate,
                self.stream_channels as i32,
            );
            if redundancy_bytes == 0 {
                redundancy = false;
            }
        }
        let bytes_target = (max_data_bytes - redundancy_bytes)
            .min(self.bitrate * frame_size as i32 / (48_000 * 8))
            - 1;
        let mut enc = RangeEncoder::new((max_data_bytes - 1).max(0) as usize);

        let mut pcm_buf = vec![0.0f32; (total_buffer + frame_size) * ch];
        pcm_buf[..total_buffer * ch].copy_from_slice(
            &self.delay_buffer[(ENCODER_BUFFER - total_buffer) * ch..ENCODER_BUFFER * ch],
        );
        let hp_freq_smth1 = if self.mode == Mode::Celt {
            hp_min_cutoff_smth_q15()
        } else {
            self.silk.variable_hp_smth1_q15()
        };
        let (smth2, cutoff_hz) = hp_smooth_cutoff(self.variable_hp_smth2_q15, hp_freq_smth1);
        self.variable_hp_smth2_q15 = smth2;
        if self.config.application == Application::Voip {
            self.hp_cutoff(pcm, cutoff_hz, &mut pcm_buf[total_buffer * ch..]);
        } else {
            self.dc_reject(pcm, &mut pcm_buf[total_buffer * ch..]);
        }
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

        let mut hb_gain = 1.0f32;
        if self.mode != Mode::Celt {
            let sm = &mut self.silk_mode;
            // The bits between SILK and CELT.
            let total_bitrate = 8 * bytes_target * frame_rate;
            if self.mode == Mode::Hybrid {
                sm.bit_rate = silk_rate_for_hybrid(
                    total_bitrate,
                    curr_bandwidth,
                    frame_size == 960,
                    vbr,
                    sm.lbrr_coded,
                    self.stream_channels as i32,
                );
                if self.energy_masking.is_none() {
                    // The fewer bits the high band gets, the quieter it is made.
                    let celt_rate = total_bitrate - sm.bit_rate;
                    hb_gain = 1.0 - celt_exp2(-celt_rate as f32 * (1.0 / 1024.0));
                }
            } else {
                sm.bit_rate = total_bitrate;
            }
            if let (Some(mask), true, false) = (self.energy_masking.as_ref(), vbr, self.lfe) {
                // Surround masking for SILK: conservatively, half the masking.
                let (end, srate) = match self.bandwidth {
                    Bandwidth::Narrow => (13, 8000.0f32),
                    Bandwidth::Medium => (15, 12000.0),
                    _ => (17, 16000.0),
                };
                let mut mask_sum = 0.0f32;
                for c in 0..ch {
                    for &m in &mask[21 * c..21 * c + end] {
                        let mut m = m.clamp(-2.0, 0.5);
                        if m > 0.0 {
                            m *= 0.5;
                        }
                        mask_sum += m;
                    }
                }
                let masking_depth = mask_sum / end as f32 * ch as f32 + 0.2;
                let rate_offset = ((srate * masking_depth) as i32).max(-2 * sm.bit_rate / 3);
                // Hybrid splits the change between SILK and CELT.
                if self.bandwidth >= Bandwidth::SuperWide {
                    sm.bit_rate += 3 * rate_offset / 5;
                } else {
                    sm.bit_rate += rate_offset;
                }
            }
            sm.payload_size_ms = (1000 * frame_size / 48_000) as i32;
            sm.n_channels_api = ch;
            sm.n_channels_internal = self.stream_channels;
            sm.desired_internal_sample_rate = match curr_bandwidth {
                Bandwidth::Narrow => 8000,
                Bandwidth::Medium => 12000,
                _ => 16000,
            };
            // No narrower band at the lowest hybrid rates.
            sm.min_internal_sample_rate = if self.mode == Mode::Hybrid {
                16000
            } else {
                8000
            };
            sm.max_internal_sample_rate = 16000;
            if self.mode == Mode::Silk {
                let mut effective_max_rate = frame_rate * max_data_bytes * 8;
                if frame_rate > 50 {
                    effective_max_rate = effective_max_rate * 2 / 3;
                }
                if effective_max_rate < 8000 {
                    sm.max_internal_sample_rate = 12000;
                    sm.desired_internal_sample_rate = sm.desired_internal_sample_rate.min(12000);
                }
                if effective_max_rate < 7000 {
                    sm.max_internal_sample_rate = 8000;
                    sm.desired_internal_sample_rate = sm.desired_internal_sample_rate.min(8000);
                }
            }
            sm.use_cbr = !vbr;
            // The packet's bits, less the TOC, the redundant frame and its signalling.
            sm.max_bits = (max_data_bytes - 1) * 8;
            if redundancy && redundancy_bytes >= 2 {
                sm.max_bits -= redundancy_bytes * 8 + 1;
                if self.mode == Mode::Hybrid {
                    sm.max_bits -= 20;
                }
            }
            if sm.use_cbr {
                if self.mode == Mode::Hybrid {
                    // CBR with CELT after it: SILK in capped VBR, which may take up to a
                    // quarter of the other bits; CELT absorbs the difference.
                    let other_bits =
                        (sm.max_bits - sm.bit_rate * frame_size as i32 / 48_000).max(0) as i16;
                    sm.max_bits = (sm.max_bits - i32::from(other_bits) * 3 / 4).max(0);
                    sm.use_cbr = false;
                }
            } else if self.mode == Mode::Hybrid {
                // Constrained: SILK's share of the most the packet may hold.
                let max_bit_rate = silk_rate_for_hybrid(
                    sm.max_bits * 48_000 / frame_size as i32,
                    curr_bandwidth,
                    frame_size == 960,
                    vbr,
                    sm.lbrr_coded,
                    self.stream_channels as i32,
                );
                sm.max_bits = max_bit_rate * frame_size as i32 / 48_000;
            }

            if let Some(p) = prefill {
                // A smooth onset, so that SILK does not code a step, placed to leave no gap
                // against the redundant CELT frame.
                let off = ch * (ENCODER_BUFFER - DELAY_COMPENSATION - 120);
                gain_fade(&mut self.delay_buffer[off..], 0.0, 1.0, 120, ch);
                self.delay_buffer[..off].fill(0.0);
                let pcm_silk: Vec<i16> = self.delay_buffer[..ENCODER_BUFFER * ch]
                    .iter()
                    .map(|&v| float2int16(v))
                    .collect();
                self.silk
                    .encode(&mut self.silk_mode, &pcm_silk, None, p, activity)?;
                // No second switch in the real encode.
                self.silk_mode.opus_can_switch = false;
            }
            let pcm_silk: Vec<i16> = pcm_buf[total_buffer * ch..]
                .iter()
                .map(|&v| float2int16(v))
                .collect();
            let n_bytes = self.silk.encode(
                &mut self.silk_mode,
                &pcm_silk,
                Some(&mut enc),
                Prefill::No,
                activity,
            )?;
            if self.mode == Mode::Silk {
                // The TOC signals SILK's internal rate.
                curr_bandwidth = match self.silk_mode.internal_sample_rate {
                    8000 => Bandwidth::Narrow,
                    12000 => Bandwidth::Medium,
                    _ => Bandwidth::Wide,
                };
            }
            self.silk_mode.opus_can_switch = self.silk_mode.switch_ready && !self.nonfinal_frame;
            if n_bytes == 0 {
                // DTX.
                self.final_range = 0;
                out.push(gen_toc(
                    self.mode,
                    frame_rate as usize,
                    curr_bandwidth,
                    self.stream_channels,
                ));
                return Ok(());
            }
            if self.silk_mode.opus_can_switch {
                // A bandwidth switch: this frame ends with a redundant CELT frame.
                redundancy_bytes = compute_redundancy_bytes(
                    max_data_bytes,
                    self.bitrate,
                    frame_rate,
                    self.stream_channels as i32,
                );
                redundancy = redundancy_bytes != 0;
                celt_to_silk = false;
                self.silk_bw_switch = true;
            }
        }

        let endband = end_band(curr_bandwidth);
        if self.mode != Mode::Silk {
            self.celt.set_prediction(2);
        }
        let mut tmp_prefill = [0.0f32; 120 * 2];
        if self.mode != Mode::Silk && self.prev_mode.is_some_and(|p| p != self.mode) {
            let at = (ENCODER_BUFFER - total_buffer - 120) * ch;
            tmp_prefill[..120 * ch].copy_from_slice(&self.delay_buffer[at..at + 120 * ch]);
        }
        if ENCODER_BUFFER > frame_size + total_buffer {
            let keep = ENCODER_BUFFER - frame_size - total_buffer;
            self.delay_buffer
                .copy_within(ch * frame_size..ch * (frame_size + keep), 0);
            self.delay_buffer[ch * keep..ch * ENCODER_BUFFER].copy_from_slice(&pcm_buf);
        } else {
            let from = (frame_size + total_buffer - ENCODER_BUFFER) * ch;
            self.delay_buffer[..ENCODER_BUFFER * ch]
                .copy_from_slice(&pcm_buf[from..from + ENCODER_BUFFER * ch]);
        }
        // The fades after the delay line: they must not reach SILK.
        if self.prev_hb_gain < 1.0 || hb_gain < 1.0 {
            gain_fade(&mut pcm_buf, self.prev_hb_gain, hb_gain, frame_size, ch);
        }
        self.prev_hb_gain = hb_gain;
        if self.mode != Mode::Hybrid || self.stream_channels == 1 {
            let e = plan.equiv_rate;
            self.silk_mode.stereo_width_q14 = if e > 32_000 {
                16384
            } else if e < 16_000 {
                0
            } else {
                16384 - 2048 * (32_000 - e) / (e - 14_000)
            };
        }
        if self.energy_masking.is_none()
            && ch == 2
            && (self.hybrid_stereo_width_q14 < 1 << 14 || self.silk_mode.stereo_width_q14 < 1 << 14)
        {
            // Narrower stereo at low rates.
            let g1 = self.hybrid_stereo_width_q14 as f32 * (1.0 / 16384.0);
            let g2 = self.silk_mode.stereo_width_q14 as f32 * (1.0 / 16384.0);
            stereo_fade(&mut pcm_buf[..frame_size * 2], g1, g2);
            self.hybrid_stereo_width_q14 = self.silk_mode.stereo_width_q14;
        }

        let hybrid = self.mode == Mode::Hybrid;
        if self.mode != Mode::Celt
            && enc.tell() + 17 + 20 * i32::from(hybrid) <= 8 * (max_data_bytes - 1)
        {
            // SILK-only: the decoder infers the redundancy from the length.
            if hybrid {
                enc.enc_bit_logp(redundancy, 12);
            }
            if redundancy {
                enc.enc_bit_logp(celt_to_silk, 1);
                // Hybrid keeps 8 bits for the length and a few for CELT.
                let max_redundancy = if hybrid {
                    (max_data_bytes - 1) - ((enc.tell() + 8 + 3 + 7) >> 3)
                } else {
                    (max_data_bytes - 1) - ((enc.tell() + 7) >> 3)
                };
                redundancy_bytes = redundancy_bytes.min(max_redundancy).clamp(2, 257);
                if hybrid {
                    enc.enc_uint((redundancy_bytes - 2) as u32, 256);
                }
            }
        } else {
            redundancy = false;
        }
        if !redundancy {
            self.silk_bw_switch = false;
            redundancy_bytes = 0;
        }
        let start_band = if self.mode != Mode::Celt { 17 } else { 0 };

        let stream_channels = self.stream_channels;
        let silk_info = SilkInfo {
            signal_type: self.silk_mode.signal_type,
            offset: self.silk_mode.offset,
        };
        let celt_frame =
            |n: usize, start: usize, bitrate: Option<i32>, vbr: bool, cvbr: bool| CeltEncodeFrame {
                frame_size: n,
                start_band: start,
                end_band: endband,
                stream_channels,
                bitrate,
                vbr,
                constrained_vbr: cvbr,
                silk_info,
            };
        if redundancy || self.mode != Mode::Silk {
            self.celt.set_analysis(&plan.analysis);
        }
        let mut redundant: Vec<u8> = vec![];
        let mut redundant_rng = 0u32;
        // The 5 ms redundant frame of a switch from CELT, first: CELT starts afresh after it.
        if redundancy && celt_to_silk {
            let mut re = RangeEncoder::new(redundancy_bytes as usize);
            let f = celt_frame(240, 0, None, false, false);
            self.celt.encode(&pcm_buf[..240 * ch], &f, &mut re)?;
            redundant_rng = re.rng();
            redundant = re
                .done()
                .ok_or(Error::Corrupt("a redundant frame overflowed"))?;
            self.celt.reset();
        }

        let mut body: Vec<u8> = vec![];
        // Over budget (SILK, unlikely; or no room left for CELT): a frame the decoder
        // conceals.
        let mut bust = false;
        if self.mode == Mode::Silk {
            let ret = ((enc.tell() + 7) >> 3) as usize;
            self.final_range = enc.rng();
            bust = enc.tell() > (max_data_bytes - 1) * 8;
            match enc.done() {
                Some(mut b) => {
                    b.truncate(ret);
                    body = b;
                }
                None => bust = true,
            }
        } else {
            let nb_compr_bytes = (max_data_bytes - 1) - redundancy_bytes;
            enc.shrink(nb_compr_bytes.max(0) as usize);
            let (bitrate, celt_vbr, cvbr) = match (vbr, hybrid) {
                // CELT refuses a bitrate of 500 or less and keeps the frame's maximum, as
                // libopus's does when SILK takes (nearly) all of it.
                (true, true) => {
                    let rate = self.bitrate - self.silk_mode.bit_rate;
                    ((rate > 500).then_some(rate), true, false)
                }
                (true, false) => (Some(self.bitrate), true, self.config.constrained_vbr),
                (false, _) => (None, false, false),
            };
            if self.prev_mode.is_some_and(|p| p != self.mode) {
                self.celt.reset();
                // Prefill from the 2.5 ms before the frame, coding nothing.
                let mut dummy = RangeEncoder::new(2);
                let f = celt_frame(120, start_band, bitrate, celt_vbr, cvbr);
                self.celt.encode(&tmp_prefill[..120 * ch], &f, &mut dummy)?;
                self.celt.set_prediction(0);
            }
            if enc.tell() <= 8 * nb_compr_bytes && nb_compr_bytes >= 2 {
                let f = celt_frame(frame_size, start_band, bitrate, celt_vbr, cvbr);
                self.celt
                    .encode(&pcm_buf[..frame_size * ch], &f, &mut enc)?;
                self.final_range = self.celt.final_range();
                match enc.done() {
                    Some(b) => body = b,
                    None => bust = true,
                }
            } else {
                bust = true;
            }
        }

        // The 5 ms redundant frame of a switch to CELT, last.
        if redundancy && !celt_to_silk {
            self.celt.reset();
            self.celt.set_prediction(0);
            let mut dummy = RangeEncoder::new(2);
            let n2 = 240;
            let n4 = 120;
            let at = ch * (frame_size - n2 - n4);
            let f = celt_frame(n4, 0, None, false, false);
            self.celt
                .encode(&pcm_buf[at..at + n4 * ch], &f, &mut dummy)?;
            let mut re = RangeEncoder::new(redundancy_bytes as usize);
            let at = ch * (frame_size - n2);
            let f = celt_frame(n2, 0, None, false, false);
            self.celt.encode(&pcm_buf[at..at + n2 * ch], &f, &mut re)?;
            redundant_rng = re.rng();
            redundant = re
                .done()
                .ok_or(Error::Corrupt("a redundant frame overflowed"))?;
        }

        let toc = gen_toc(
            self.mode,
            frame_rate as usize,
            curr_bandwidth,
            self.stream_channels,
        );
        self.final_range ^= redundant_rng;
        self.prev_mode = Some(if plan.to_celt { Mode::Celt } else { self.mode });
        self.prev_channels = self.stream_channels;
        self.first = false;

        // The general DTX: inactivity (or digital silence) after 200 ms of it is sent as TOC
        // bytes.
        if self.config.dtx && (plan.analysis.valid || plan.is_silence) && activity == Some(false) {
            self.nb_no_activity_ms_q1 += (2 * 1000 * frame_size / 48_000) as i32;
            let q1 = self.nb_no_activity_ms_q1;
            if q1 > NB_SPEECH_FRAMES_BEFORE_DTX * 20 * 2 {
                if q1 <= (NB_SPEECH_FRAMES_BEFORE_DTX + MAX_CONSECUTIVE_DTX) * 20 * 2 {
                    self.final_range = 0;
                    out.push(toc);
                    return Ok(());
                }
                self.nb_no_activity_ms_q1 = NB_SPEECH_FRAMES_BEFORE_DTX * 20 * 2;
            }
        } else {
            self.nb_no_activity_ms_q1 = 0;
        }

        out.push(toc);
        if bust {
            if max_data_bytes < 2 {
                return Err(Error::BadArgument("no room for a frame"));
            }
            out.push(0);
            self.final_range = 0;
        } else {
            if self.mode == Mode::Silk && !redundancy {
                // Trailing zeros of a SILK-only frame are what the range decoder fills in.
                while body.len() > 2 && body.last() == Some(&0) {
                    body.pop();
                }
            }
            out.extend_from_slice(&body);
            out.extend_from_slice(&redundant);
        }
        if !vbr {
            pad(out, max_data_bytes as usize)?;
        }
        Ok(())
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

    /// `hp_cutoff`: the second-order high-pass at `cutoff_hz` (`silk_biquad_float`, direct
    /// form II transposed) from `pcm` into `out`.
    fn hp_cutoff(&mut self, pcm: &[f32], cutoff_hz: i32, out: &mut [f32]) {
        let (b_q28, a_q28) = hp_cutoff_coefs(cutoff_hz);
        let q = 1.0f32 / (1u32 << 28) as f32;
        let a = [a_q28[0] as f32 * q, a_q28[1] as f32 * q];
        let b = [
            b_q28[0] as f32 * q,
            b_q28[1] as f32 * q,
            b_q28[2] as f32 * q,
        ];
        let ch = self.channels;
        for c in 0..ch {
            let (mut s0, mut s1) = (self.hp_mem[2 * c], self.hp_mem[2 * c + 1]);
            for (o, &x) in out
                .iter_mut()
                .skip(c)
                .step_by(ch)
                .zip(pcm.iter().skip(c).step_by(ch))
            {
                let vout = s0 + b[0] * x;
                s0 = s1 - vout * a[0] + b[1] * x;
                s1 = -vout * a[1] + b[2] * x + 1e-30;
                *o = vout;
            }
            self.hp_mem[2 * c] = s0;
            self.hp_mem[2 * c + 1] = s1;
        }
    }
}

#[cfg(test)]
#[path = "encoder_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "encoder_modes_tests.rs"]
mod modes_tests;

#[cfg(test)]
#[path = "encoder_analysis_tests.rs"]
mod analysis_tests;

#[cfg(test)]
#[path = "encoder_depth_tests.rs"]
mod depth_tests;
