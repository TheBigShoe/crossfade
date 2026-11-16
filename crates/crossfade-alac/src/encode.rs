//! The packet encoder: one frame of every channel in, one packet out, the inverse of
//! [`Decoder`](crate::Decoder).
//!
//! Each element is coded three ways at most and the smallest kept: predicted with the
//! channels as they are, predicted as mid and side (a pair only), or not compressed at all
//! (the format's escape), so a packet is never larger than its samples and a few header bytes.
//! The predictor starts every packet from the reference encoder's initial coefficients, so
//! packets do not depend on one another and the same input always gives the same bytes.

use crate::bits::sign_extend;
use crate::write::{BitWriter, predict, put_residuals};
use crate::{Config, Error, Result};

const SCE: u64 = 0;
const CPE: u64 = 1;
const LFE: u64 = 3;
const END: u64 = 7;

/// The predictor's order and the scale of its coefficients (the reference encoder's).
const ORDER: usize = 8;
const DEN_SHIFT: u32 = 9;
/// The Rice coder's history multiplier, in quarters: unchanged.
const PB_FACTOR: u32 = 4;
/// Mid/side as the format's mix: `u = (res * l + (4 - res) * r) >> 2`, `v = l - r`.
const MIX_BITS: u32 = 2;
const MIX_RES: i32 = 2;

/// The elements of a packet for each channel count, in ALAC's channel order: `true` is a
/// channel pair. From six channels up the last single element is the LFE.
const ELEMENTS: [&[bool]; 8] = [
    &[false],
    &[true],
    &[false, true],
    &[false, true, false],
    &[false, true, true],
    &[false, true, true, false],
    &[false, true, true, false, false],
    &[false, true, true, true, false],
];

/// The reference encoder's starting coefficients (`init_coefs`): a gentle low-pass predictor.
fn initial_coefs() -> [i16; ORDER] {
    let mut c = [0i16; ORDER];
    c[0] = ((38 * (1 << DEN_SHIFT)) >> 4) as i16;
    c[1] = ((-29 * (1 << DEN_SHIFT)) >> 4) as i16;
    c[2] = ((-2 * (1 << DEN_SHIFT)) >> 4) as i16;
    c
}

/// Encodes the packets of one stream.
#[derive(Debug, Clone)]
pub struct Encoder {
    config: Config,
    u: Vec<i32>,
    v: Vec<i32>,
}

impl Encoder {
    /// An encoder for the stream `config` describes ([`Config::new`] for the usual one).
    pub fn new(config: Config) -> Result<Self> {
        config.check()?;
        let n = config.frame_length as usize;
        Ok(Self {
            config,
            u: Vec::with_capacity(n),
            v: Vec::with_capacity(n),
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Encodes one packet from `pcm`: 1 to `frame_length` sample frames, interleaved in
    /// ALAC's channel order (see [`wave_layout`](crate::wave_layout)), each sample
    /// sign-extended and right-justified at the stream's bit depth. The packet is appended to
    /// `out`; on an error nothing is.
    pub fn encode(&mut self, pcm: &[i32], out: &mut Vec<u8>) -> Result<()> {
        let channels = usize::from(self.config.channels);
        let depth = u32::from(self.config.bit_depth);
        if pcm.is_empty() || !pcm.len().is_multiple_of(channels) {
            return Err(Error::BadInput("samples that are not whole frames"));
        }
        let num = pcm.len() / channels;
        if num > self.config.frame_length as usize {
            return Err(Error::BadInput("more frames than a packet holds"));
        }
        if pcm.iter().any(|&s| sign_extend(s, depth) != s) {
            return Err(Error::BadInput("a sample wider than the bit depth"));
        }
        let mut w = BitWriter::default();
        let mut ch = 0;
        let (mut singles, mut pairs) = (0u64, 0u64);
        let elements = ELEMENTS[channels - 1];
        for (e, &pair) in elements.iter().enumerate() {
            if pair {
                w.put(3, CPE);
                w.put(4, pairs);
                pairs += 1;
                self.pair(&mut w, pcm, ch, num);
                ch += 2;
            } else {
                let lfe = channels >= 6 && e + 1 == elements.len();
                w.put(3, if lfe { LFE } else { SCE });
                w.put(4, if lfe { 0 } else { singles });
                singles += u64::from(!lfe);
                self.mono(&mut w, pcm, ch, num);
                ch += 1;
            }
        }
        w.put(3, END);
        w.align();
        out.extend_from_slice(&w.bytes);
        Ok(())
    }

    /// The rest of an element header: 12 unused bits, then partial frame, shift and escape.
    fn header(&self, w: &mut BitWriter, num: usize, shift_bytes: u32, escape: bool) {
        let partial = num != self.config.frame_length as usize;
        w.put(12, 0);
        w.put(1, u64::from(partial));
        w.put(2, u64::from(shift_bytes));
        w.put(1, u64::from(escape));
        if partial {
            w.put(32, num as u64);
        }
    }

    /// Bytes of each sample kept out of the predictor and stored as they are, as the
    /// reference encoder does: the low byte at 24 bits, the low two at 32.
    fn shift_bytes(&self) -> u32 {
        match self.config.bit_depth {
            24 => 1,
            32 => 2,
            _ => 0,
        }
    }

    /// One channel's predictor header: its mode, scale, order and starting coefficients.
    fn channel_head(w: &mut BitWriter) {
        w.put(4, 0); // mode: no prediction on prediction
        w.put(4, u64::from(DEN_SHIFT));
        w.put(3, u64::from(PB_FACTOR));
        w.put(5, ORDER as u64);
        for c in initial_coefs() {
            w.put(16, u64::from(c as u16));
        }
    }

    /// One channel's residuals from the predictor [`channel_head`](Self::channel_head)
    /// describes.
    fn channel_residuals(&self, w: &mut BitWriter, samples: &[i32], chan_bits: u32) {
        let mut coefs = initial_coefs();
        let residuals = predict(samples, &mut coefs, chan_bits, DEN_SHIFT);
        let k = &self.config;
        put_residuals(
            w,
            &residuals,
            u32::from(k.mb),
            u32::from(k.pb) * PB_FACTOR / 4,
            u32::from(k.kb),
            chan_bits,
        );
    }

    fn mono(&mut self, w: &mut BitWriter, pcm: &[i32], ch: usize, num: usize) {
        let channels = usize::from(self.config.channels);
        let depth = u32::from(self.config.bit_depth);
        let shift_bytes = self.shift_bytes();
        let shift = shift_bytes * 8;
        let samples = pcm.iter().skip(ch).step_by(channels);
        self.u.clear();
        self.u.extend(samples.clone().map(|&s| s >> shift));

        let mut packed = BitWriter::default();
        self.header(&mut packed, num, shift_bytes, false);
        packed.put(16, 0); // mixing parameters, unused by one channel
        Self::channel_head(&mut packed);
        if shift != 0 {
            for &s in samples.clone() {
                packed.put(shift, (s as u32 & ((1 << shift) - 1)).into());
            }
        }
        self.channel_residuals(&mut packed, &self.u, depth - shift);

        let raw_bits =
            16 + 32 * usize::from(num != self.config.frame_length as usize) + num * depth as usize;
        if packed.len() < raw_bits {
            w.append(&packed);
        } else {
            self.header(w, num, 0, true);
            for &s in samples {
                w.put(depth, u64::from(s as u32) & ((1u64 << depth) - 1));
            }
        }
    }

    fn pair(&mut self, w: &mut BitWriter, pcm: &[i32], ch: usize, num: usize) {
        let channels = usize::from(self.config.channels);
        let depth = u32::from(self.config.bit_depth);
        let shift_bytes = self.shift_bytes();
        let shift = shift_bytes * 8;
        let frames = pcm.chunks_exact(channels).map(|f| (f[ch], f[ch + 1]));
        // The side channel needs one bit more than the samples.
        let chan_bits = depth - shift + 1;

        let mut best: Option<BitWriter> = None;
        for mix_res in [0, MIX_RES] {
            self.u.clear();
            self.v.clear();
            for (l, r) in frames.clone() {
                let (l, r) = (l >> shift, r >> shift);
                if mix_res == 0 {
                    self.u.push(l);
                    self.v.push(r);
                } else {
                    let rest = (1 << MIX_BITS) - mix_res;
                    self.u.push((mix_res * l + rest * r) >> MIX_BITS);
                    self.v.push(l - r);
                }
            }
            let mut packed = BitWriter::default();
            self.header(&mut packed, num, shift_bytes, false);
            packed.put(8, u64::from(MIX_BITS));
            packed.put(8, u64::from(mix_res as u8));
            Self::channel_head(&mut packed);
            Self::channel_head(&mut packed);
            if shift != 0 {
                let mask = (1u32 << shift) - 1;
                for (l, r) in frames.clone() {
                    packed.put(shift, (l as u32 & mask).into());
                    packed.put(shift, (r as u32 & mask).into());
                }
            }
            self.channel_residuals(&mut packed, &self.u, chan_bits);
            self.channel_residuals(&mut packed, &self.v, chan_bits);
            if best.as_ref().is_none_or(|b| packed.len() < b.len()) {
                best = Some(packed);
            }
        }
        let raw_bits = 16
            + 32 * usize::from(num != self.config.frame_length as usize)
            + 2 * num * depth as usize;
        match best {
            Some(packed) if packed.len() < raw_bits => w.append(&packed),
            _ => {
                self.header(w, num, 0, true);
                let mask = (1u64 << depth) - 1;
                for (l, r) in frames {
                    w.put(depth, u64::from(l as u32) & mask);
                    w.put(depth, u64::from(r as u32) & mask);
                }
            }
        }
    }
}
