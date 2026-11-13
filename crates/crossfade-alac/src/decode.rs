//! The packet decoder: one packet (one frame of every channel) in, interleaved samples out.
//!
//! A packet is a sequence of elements, each led by a 3-bit tag: single channel (SCE) and LFE
//! elements carry one channel, channel pair (CPE) elements two, in the order of the stream's
//! channel layout; data stream (DSE) and fill (FIL) elements carry nothing to decode and are
//! skipped; an END element, or the last channel, ends the packet. Coupling channel (CCE) and
//! program config (PCE) elements have no length a decoder could skip by, and no ALAC encoder
//! writes them: they are refused.

use crate::bits::{Bits, sign_extend};
use crate::predictor::{integrate, unpredict};
use crate::rice::{self, Params};
use crate::{Config, Error, Result};

const SCE: u32 = 0;
const CPE: u32 = 1;
const CCE: u32 = 2;
const LFE: u32 = 3;
const DSE: u32 = 4;
const PCE: u32 = 5;
const FIL: u32 = 6;
const END: u32 = 7;

/// Decodes the packets of one stream. Packets are independent: any packet can be decoded at
/// any time, so there is nothing to reset after a seek.
#[derive(Debug, Clone)]
pub struct Decoder {
    config: Config,
    residual: Vec<i32>,
    u: Vec<i32>,
    v: Vec<i32>,
}

/// One channel's predictor parameters from a compressed element's header.
struct Channel {
    mode: u32,
    den_shift: u32,
    pb_factor: u32,
    coefs: [i16; 32],
    order: usize,
}

impl Channel {
    fn read(bits: &mut Bits<'_>) -> Result<Self> {
        let a = bits.read(8)?;
        let b = bits.read(8)?;
        let order = (b & 0x1F) as usize;
        let mut coefs = [0i16; 32];
        for c in &mut coefs[..order] {
            *c = bits.read(16)? as u16 as i16;
        }
        Ok(Self {
            mode: a >> 4,
            den_shift: a & 0xF,
            pb_factor: b >> 5,
            coefs,
            order,
        })
    }
}

/// What every element header carries.
struct Header {
    num: usize,
    shift_bytes: u32,
    escape: bool,
}

impl Decoder {
    /// A decoder for the stream `config` describes. Its buffers, three of `frame_length`
    /// samples, are allocated here and never grow.
    pub fn new(config: Config) -> Result<Self> {
        config.check()?;
        let n = config.frame_length as usize;
        Ok(Self {
            config,
            residual: vec![0; n],
            u: vec![0; n],
            v: vec![0; n],
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Decodes one packet into `out` (cleared first): interleaved samples in ALAC's channel
    /// order (see [`wave_layout`](crate::wave_layout) for WAVE's), sign-extended and
    /// right-justified at the stream's bit depth. Returns the sample frames decoded: the
    /// config's frame length, or fewer for a packet that says so (the last one, usually).
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<i32>) -> Result<usize> {
        out.clear();
        let result = self.decode_into(packet, out);
        match result {
            Ok(frames) => {
                out.truncate(frames * usize::from(self.config.channels));
                Ok(frames)
            }
            Err(e) => {
                out.clear();
                Err(e)
            }
        }
    }

    fn decode_into(&mut self, packet: &[u8], out: &mut Vec<i32>) -> Result<usize> {
        let channels = usize::from(self.config.channels);
        out.resize(self.config.frame_length as usize * channels, 0);
        let mut bits = Bits::new(packet);
        let mut ch = 0;
        let mut frames = None;
        while ch < channels {
            match bits.read(3)? {
                SCE | LFE => {
                    self.mono(&mut bits, ch, &mut frames, out)?;
                    ch += 1;
                }
                CPE => {
                    if ch + 2 > channels {
                        return Err(Error::Malformed("more channels than the config has"));
                    }
                    self.pair(&mut bits, ch, &mut frames, out)?;
                    ch += 2;
                }
                DSE => skip_data_stream(&mut bits)?,
                FIL => skip_fill(&mut bits)?,
                END => break,
                CCE => return Err(Error::Unsupported("a coupling channel element")),
                PCE => return Err(Error::Unsupported("a program config element")),
                _ => unreachable!("a 3-bit tag"),
            }
        }
        if ch < channels {
            return Err(Error::Malformed("packet ends before every channel"));
        }
        Ok(frames.unwrap_or(0))
    }

    /// The element header after the tag; every element of a packet must give the same length.
    fn header(&self, bits: &mut Bits<'_>, frames: &mut Option<usize>) -> Result<Header> {
        bits.skip(4)?; // element instance tag
        if bits.read(12)? != 0 {
            return Err(Error::Malformed("reserved header bits are set"));
        }
        let flags = bits.read(4)?;
        let partial = flags & 0x8 != 0;
        let num = if partial {
            bits.read(32)? as usize
        } else {
            self.config.frame_length as usize
        };
        if num > self.config.frame_length as usize {
            return Err(Error::Malformed(
                "frame longer than the config's frame length",
            ));
        }
        if frames.is_some_and(|f| f != num) {
            return Err(Error::Malformed("elements of one packet differ in length"));
        }
        *frames = Some(num);
        let shift_bytes = (flags >> 1) & 3;
        if shift_bytes * 8 >= u32::from(self.config.bit_depth) {
            return Err(Error::Malformed("shifted bytes leave no bits to code"));
        }
        Ok(Header {
            num,
            shift_bytes,
            escape: flags & 1 != 0,
        })
    }

    /// Residuals for one channel, then the predictor over them, into `dest`.
    fn rebuild(
        &mut self,
        bits: &mut Bits<'_>,
        c: &mut Channel,
        chan_bits: u32,
        num: usize,
        second: bool,
    ) -> Result<()> {
        let k = &self.config;
        let params = Params {
            mb: u32::from(k.mb),
            pb: u32::from(k.pb) * c.pb_factor / 4,
            kb: u32::from(k.kb),
        };
        let residual = &mut self.residual[..num];
        rice::decode(bits, params, chan_bits, residual)?;
        if c.mode != 0 {
            // Prediction on prediction: the residuals were first-differenced once more.
            integrate(residual, chan_bits);
        }
        let dest = if second { &mut self.v } else { &mut self.u };
        unpredict(
            residual,
            &mut dest[..num],
            &mut c.coefs[..c.order],
            chan_bits,
            c.den_shift,
        );
        Ok(())
    }

    fn mono(
        &mut self,
        bits: &mut Bits<'_>,
        ch: usize,
        frames: &mut Option<usize>,
        out: &mut [i32],
    ) -> Result<()> {
        let Header {
            num,
            mut shift_bytes,
            escape,
        } = self.header(bits, frames)?;
        let depth = u32::from(self.config.bit_depth);
        let chan_bits = depth - shift_bytes * 8;
        let mut shift_pos = 0;
        if escape {
            for s in &mut self.u[..num] {
                *s = bits.read_signed(chan_bits)?;
            }
            shift_bytes = 0;
        } else {
            bits.skip(16)?; // mixing parameters, unused by one channel
            let mut c = Channel::read(bits)?;
            shift_pos = bits.position();
            bits.skip(shift_bytes as usize * 8 * num)?;
            self.rebuild(bits, &mut c, chan_bits, num, false)?;
        }
        let shift = shifted(depth, shift_bytes);
        let mut low = bits.at(shift_pos);
        let channels = usize::from(self.config.channels);
        for (i, &u) in self.u[..num].iter().enumerate() {
            let mut s = u;
            if shift != 0 {
                s = (s << shift) | low.read(shift)? as i32;
            }
            out[i * channels + ch] = sign_extend(s, depth);
        }
        Ok(())
    }

    fn pair(
        &mut self,
        bits: &mut Bits<'_>,
        ch: usize,
        frames: &mut Option<usize>,
        out: &mut [i32],
    ) -> Result<()> {
        let Header {
            num,
            mut shift_bytes,
            escape,
        } = self.header(bits, frames)?;
        let depth = u32::from(self.config.bit_depth);
        let (mut mix_bits, mut mix_res) = (0, 0);
        let mut shift_pos = 0;
        if escape {
            for i in 0..num {
                self.u[i] = bits.read_signed(depth)?;
                self.v[i] = bits.read_signed(depth)?;
            }
            shift_bytes = 0;
        } else {
            // The side channel needs one bit more than the samples.
            let chan_bits = depth - shift_bytes * 8 + 1;
            mix_bits = bits.read(8)?;
            mix_res = i32::from(bits.read(8)? as u8 as i8);
            let mut cu = Channel::read(bits)?;
            let mut cv = Channel::read(bits)?;
            shift_pos = bits.position();
            bits.skip(shift_bytes as usize * 16 * num)?;
            self.rebuild(bits, &mut cu, chan_bits, num, false)?;
            self.rebuild(bits, &mut cv, chan_bits, num, true)?;
        }
        let shift = shifted(depth, shift_bytes);
        let mut low = bits.at(shift_pos);
        let channels = usize::from(self.config.channels);
        let pairs = self.u[..num].iter().zip(&self.v[..num]);
        for (i, (&u, &v)) in pairs.enumerate() {
            let (mut l, mut r) = if mix_res != 0 {
                let l = u
                    .wrapping_add(v)
                    .wrapping_sub(mix_res.wrapping_mul(v) >> mix_bits.min(31));
                (l, l.wrapping_sub(v))
            } else {
                (u, v)
            };
            if shift != 0 {
                l = (l << shift) | low.read(shift)? as i32;
                r = (r << shift) | low.read(shift)? as i32;
            }
            out[i * channels + ch] = sign_extend(l, depth);
            out[i * channels + ch + 1] = sign_extend(r, depth);
        }
        Ok(())
    }
}

/// The bits a decoder puts back under each sample. Only 24- and 32-bit streams use them, as in
/// the reference decoder; at 16 and 20 bits they are read past and dropped.
fn shifted(depth: u32, shift_bytes: u32) -> u32 {
    if depth >= 24 { shift_bytes * 8 } else { 0 }
}

fn skip_data_stream(bits: &mut Bits<'_>) -> Result<()> {
    bits.skip(4)?; // element instance tag
    let aligned = bits.read(1)? != 0;
    let mut count = bits.read(8)?;
    if count == 255 {
        count += bits.read(8)?;
    }
    if aligned {
        bits.align();
    }
    bits.skip(count as usize * 8)
}

fn skip_fill(bits: &mut Bits<'_>) -> Result<()> {
    let mut count = bits.read(4)?;
    if count == 15 {
        count = 14 + bits.read(8)?;
    }
    bits.skip(count as usize * 8)
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
