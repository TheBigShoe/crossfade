//! The packet encoder: one frame of every channel in, one packet out.
//!
//! Each channel runs through the adaptive FIR predictor and the adaptive Rice coder, the same
//! arithmetic the decoder undoes. For every element the encoder tries each predictor order
//! Apple's encoder tries and each number of low bytes it may pass through uncoded, counts the
//! bits each would take on the whole frame, and writes the smallest, or the samples as they
//! are when that is no smaller.

use crate::bits::{BitWriter, Count};
use crate::decode::{CPE, END, SCE};
use crate::predictor::predict;
use crate::rice::{self, Params};
use crate::{Config, Error, Result};

/// The frame length Apple's encoder uses.
pub const DEFAULT_FRAME_LENGTH: u32 = 4096;

/// The predictor orders tried for every channel, Apple's.
const ORDERS: [usize; 2] = [4, 8];
const MAX_ORDER: usize = 8;
const DEN_SHIFT: u32 = 9;
/// The rice coder's tuning, Apple's defaults, which `afconvert` writes in every cookie.
const PB: u8 = 40;
const MB: u8 = 10;
const KB: u8 = 14;
const MAX_RUN: u16 = 255;
/// 4 leaves the config's `pb` as it is.
const PB_FACTOR: u32 = 4;
const PARAMS: Params = Params {
    mb: MB as u32,
    pb: PB as u32 * PB_FACTOR / 4,
    kb: KB as u32,
};
/// Passes over the start of a frame that let the coefficients settle before they are sent.
const CONVERGE_PASSES: usize = 8;
const CONVERGE_SHARE: usize = 32;

/// Coefficients for each order tried.
type Coefs = [[i16; MAX_ORDER]; ORDERS.len()];

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
    /// Per channel, the coefficients the last packet ended with: carried on, they start each
    /// frame closer to the signal than fresh ones would (Apple's encoder does the same).
    coefs: Vec<Coefs>,
    work: Work,
}

/// Buffers of `frame_length` samples, allocated once.
#[derive(Debug, Clone)]
struct Work {
    u: Vec<i32>,
    low: Vec<u32>,
    residual: Vec<i32>,
}

/// How one channel's predictor input codes best.
#[derive(Debug, Clone, Copy)]
struct Coding {
    /// The predictor header, its coefficients and the rice-coded residuals.
    bits: u64,
    order: usize,
    /// The coefficients sent in the header.
    sent: [i16; MAX_ORDER],
    /// Per order, the coefficients after the whole frame, for the next packet.
    after: Coefs,
}

impl Encoder {
    pub fn new(settings: EncoderConfig) -> Result<Self> {
        if settings.sample_rate == 0 {
            return Err(Error::InvalidConfig("sample rate is 0"));
        }
        let config = Config {
            frame_length: settings.frame_length,
            compatible_version: 0,
            bit_depth: settings.bit_depth,
            pb: PB,
            mb: MB,
            kb: KB,
            channels: settings.channels,
            max_run: MAX_RUN,
            max_frame_bytes: 0,
            avg_bit_rate: 0,
            sample_rate: settings.sample_rate,
        };
        config.check()?;
        let n = config.frame_length as usize;
        Ok(Self {
            config,
            bytes: 0,
            packets: 0,
            ended: false,
            coefs: vec![[initial_coefs(); ORDERS.len()]; usize::from(config.channels)],
            work: Work {
                u: vec![0; n],
                low: vec![0; n],
                residual: vec![0; n],
            },
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
            w.put(3, tag);
            w.put(4, 0); // element instance tag
            if tag == CPE {
                self.escape(&mut w, pcm, ch, 2, frames);
                ch += 2;
            } else {
                self.mono(&mut w, pcm, ch, frames);
                ch += 1;
            }
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

    /// The element header after the tag: the bytes shifted off each sample, the escape flag,
    /// and the length when it is short.
    fn header(&self, w: &mut BitWriter<'_>, frames: usize, shift: u32, escape: bool) {
        let partial = frames < self.config.frame_length as usize;
        w.put(12, 0);
        w.put(4, u32::from(partial) << 3 | shift << 1 | u32::from(escape));
        if partial {
            w.put(32, frames as u32);
        }
    }

    fn header_bits(&self, frames: usize) -> u64 {
        if frames < self.config.frame_length as usize {
            48
        } else {
            16
        }
    }

    /// One channel (a single channel element): the shift and order that code smallest, or
    /// the samples as they are.
    fn mono(&mut self, w: &mut BitWriter<'_>, pcm: &[i32], ch: usize, frames: usize) {
        let depth = u32::from(self.config.bit_depth);
        let channels = usize::from(self.config.channels);
        let mut best: Option<(u64, u32, Coding)> = None;
        for &shift in mono_shifts(depth) {
            let Work { u, low, residual } = &mut self.work;
            split(
                pcm,
                channels,
                ch,
                shift,
                &mut u[..frames],
                &mut low[..frames],
            );
            let chan_bits = depth - shift * 8;
            let Some(c) = search(&u[..frames], &self.coefs[ch], chan_bits, residual) else {
                continue;
            };
            // Header, the unused mixing bytes, the shifted bytes and the coded channel.
            let bits = self.header_bits(frames) + 16 + u64::from(shift * 8) * frames as u64;
            let total = bits + c.bits;
            if best.is_none_or(|(t, _, _)| total < t) {
                best = Some((total, shift, c));
            }
        }
        let escape = self.header_bits(frames) + u64::from(depth) * frames as u64;
        let Some((total, shift, c)) = best else {
            self.escape(w, pcm, ch, 1, frames);
            return;
        };
        self.coefs[ch] = c.after;
        if total >= escape {
            self.escape(w, pcm, ch, 1, frames);
            return;
        }
        self.header(w, frames, shift, false);
        w.put(16, 0); // mixing parameters, unused by one channel
        put_predictor(w, &c);
        let Work { u, low, residual } = &mut self.work;
        let (u, low, residual) = (
            &mut u[..frames],
            &mut low[..frames],
            &mut residual[..frames],
        );
        split(pcm, channels, ch, shift, u, low);
        if shift != 0 {
            for &l in low.iter() {
                w.put(shift * 8, l);
            }
        }
        let mut coefs = c.sent;
        let chan_bits = depth - shift * 8;
        predict(u, residual, &mut coefs[..c.order], chan_bits, DEN_SHIFT);
        rice::encode(w, PARAMS, chan_bits, residual);
    }

    /// An uncompressed element: every sample at the full depth, interleaved within a pair.
    fn escape(&self, w: &mut BitWriter<'_>, pcm: &[i32], ch: usize, width: usize, frames: usize) {
        self.header(w, frames, 0, true);
        let depth = u32::from(self.config.bit_depth);
        let channels = usize::from(self.config.channels);
        for frame in pcm.chunks_exact(channels).take(frames) {
            for &s in &frame[ch..ch + width] {
                w.put(depth, s as u32);
            }
        }
    }
}

/// Channel `ch` of `pcm` with `shift` bytes off the bottom into `u`, and those bytes into
/// `low`.
fn split(pcm: &[i32], channels: usize, ch: usize, shift: u32, u: &mut [i32], low: &mut [u32]) {
    let bits = shift * 8;
    let mask = ((1u64 << bits) - 1) as u32;
    let samples = pcm.chunks_exact(channels).map(|f| f[ch]);
    for ((u, low), s) in u.iter_mut().zip(low.iter_mut()).zip(samples) {
        *u = s >> bits;
        *low = s as u32 & mask;
    }
}

/// The order that codes `u` smallest, each order starting from the coefficients `from`.
/// `None` when no order can code it.
fn search(u: &[i32], from: &Coefs, chan_bits: u32, residual: &mut [i32]) -> Option<Coding> {
    let residual = &mut residual[..u.len()];
    let mut after = *from;
    let mut best: Option<(u64, usize, [i16; MAX_ORDER])> = None;
    for (i, &order) in ORDERS.iter().enumerate() {
        let coefs = &mut after[i][..order];
        converge(u, residual, coefs, chan_bits);
        let mut sent = [0; MAX_ORDER];
        sent[..order].copy_from_slice(coefs);
        predict(u, residual, coefs, chan_bits, DEN_SHIFT);
        // A 32-bit residual of -2^31 would not survive the rice coder's sign folding.
        if chan_bits >= 32 && residual.contains(&i32::MIN) {
            continue;
        }
        let mut count = Count::default();
        rice::encode(&mut count, PARAMS, chan_bits, residual);
        let bits = 16 + 16 * order as u64 + count.0;
        if best.is_none_or(|(b, _, _)| bits < b) {
            best = Some((bits, i, sent));
        }
    }
    best.map(|(bits, i, sent)| Coding {
        bits,
        order: ORDERS[i],
        sent,
        after,
    })
}

/// A few passes of the predictor over the start of the frame, adapting `coefs`.
fn converge(u: &[i32], scratch: &mut [i32], coefs: &mut [i16], chan_bits: u32) {
    let n = u.len() / CONVERGE_SHARE;
    if n <= coefs.len() + 1 {
        return;
    }
    for _ in 0..CONVERGE_PASSES {
        predict(&u[..n], &mut scratch[..n], coefs, chan_bits, DEN_SHIFT);
    }
}

/// A channel's predictor header: mode 0 (plain prediction), the denominator shift, the rice
/// coder's `pb` factor, the order and the coefficients.
fn put_predictor(w: &mut BitWriter<'_>, c: &Coding) {
    w.put(8, DEN_SHIFT);
    w.put(8, PB_FACTOR << 5 | c.order as u32);
    for &coef in &c.sent[..c.order] {
        w.put(16, u32::from(coef as u16));
    }
}

/// The shifts (bytes passed through below what is coded) worth trying for one channel. Apple
/// shifts one byte off 24-bit samples and two off 32-bit ones; none can be better, when the
/// low bytes are not noise (16-bit audio in a 24-bit stream, say).
fn mono_shifts(depth: u32) -> &'static [u32] {
    match depth {
        24 => &[0, 1],
        32 => &[0, 1, 2],
        _ => &[0],
    }
}

/// Apple's starting coefficients for every order.
fn initial_coefs() -> [i16; MAX_ORDER] {
    let den = 1i32 << DEN_SHIFT;
    let mut c = [0; MAX_ORDER];
    c[0] = ((38 * den) >> 4) as i16;
    c[1] = ((-29 * den) >> 4) as i16;
    c[2] = ((-2 * den) >> 4) as i16;
    c
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
