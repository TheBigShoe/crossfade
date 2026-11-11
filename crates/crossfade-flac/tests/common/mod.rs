//! Shared helpers: drive an encoder over a signal, then check the output with the decoder.
#![allow(dead_code, unreachable_pub)]

use crossfade_flac::frame::{self, ChannelAssignment};
use crossfade_flac::streaminfo::StreamInfoBlock;
use crossfade_flac::{Decoder, Encoder, EncoderConfig, Verification};

pub struct Encoded {
    pub config: EncoderConfig,
    pub bytes: Vec<u8>,
    /// Byte ranges of the frames in `bytes`.
    pub frames: Vec<(usize, usize)>,
    pub info: StreamInfoBlock,
    pub block_size: usize,
}

/// Encodes `samples` (interleaved) in blocks of `block` sample frames; 0 means the encoder's
/// own block size.
pub fn encode(config: EncoderConfig, samples: &[i32], block: usize) -> Encoded {
    let mut enc = Encoder::new(config).unwrap();
    let block = if block == 0 { enc.block_size() } else { block };
    let per = block * usize::from(config.channels);
    let mut bytes = Vec::new();
    let mut frames = Vec::new();
    for chunk in samples.chunks(per) {
        let start = bytes.len();
        enc.encode_block(chunk, &mut bytes).unwrap();
        frames.push((start, bytes.len()));
    }
    let info = enc.finish();
    Encoded {
        config,
        bytes,
        frames,
        info,
        block_size: enc.block_size(),
    }
}

/// Decodes every frame, checking headers, frame numbers, the sample total and the MD5.
pub fn decode(e: &Encoded) -> Vec<i32> {
    let mut dec = Decoder::new(&e.info).unwrap();
    let mut out = Vec::new();
    let mut scratch = Vec::new();
    for (n, &(s, t)) in e.frames.iter().enumerate() {
        let data = &e.bytes[s..t];
        let h = frame::parse_header(data).unwrap();
        assert_eq!(h.number, n as u64, "frame numbers are consecutive");
        assert!(!h.variable_block_size);
        assert_eq!(h.channels.channels(), e.config.channels);
        let block = dec.decode_frame(data).unwrap();
        block.interleave_i32(0, &mut scratch);
        out.extend_from_slice(&scratch);
    }
    assert_eq!(dec.finish().unwrap(), Verification::Md5Matched);
    out
}

pub fn round_trip(config: EncoderConfig, samples: &[i32], block: usize) -> Encoded {
    let e = encode(config, samples, block);
    assert_eq!(decode(&e), samples);
    e
}

/// Channel assignment the frame at `index` chose.
pub fn assignment(e: &Encoded, index: usize) -> ChannelAssignment {
    let (s, t) = e.frames[index];
    frame::parse_header(&e.bytes[s..t]).unwrap().channels
}

/// The never-worse-than-raw bound for one frame of `block` sample frames.
pub fn raw_bound(config: EncoderConfig, block: usize) -> usize {
    let ch = usize::from(config.channels);
    let bits = usize::from(config.bits_per_sample);
    // Header (at most 16 bytes), CRC-16, plus a verbatim subframe per channel.
    16 + 2 + ch * (1 + (block * bits).div_ceil(8)) + 1
}

pub fn assert_within_bound(e: &Encoded) {
    for (i, &(s, t)) in e.frames.iter().enumerate() {
        let block = if i + 1 == e.frames.len() {
            // Unknown here; the full block size is an upper bound for the last one too.
            e.block_size
        } else {
            e.block_size
        };
        assert!(
            t - s <= raw_bound(e.config, block),
            "frame {i} is {} bytes, bound {}",
            t - s,
            raw_bound(e.config, block)
        );
    }
}

/// Small seeded generator (xorshift) so tests need no dependencies.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform value that fits `bits` signed bits.
    pub fn sample(&mut self, bits: u8) -> i32 {
        (self.next() as i64 >> (64 - u32::from(bits))) as i32
    }
}

pub fn noise(bits: u8, count: usize, seed: u64) -> Vec<i32> {
    let mut r = Rng(seed | 1);
    (0..count).map(|_| r.sample(bits)).collect()
}

pub const KINDS: u8 = 10;

/// `count` interleaved samples of one of several shapes that stress different tools.
pub fn signal(kind: u8, bits: u8, count: usize, channels: usize, seed: u64) -> Vec<i32> {
    let mut r = Rng(seed | 1);
    let (lo, hi) = if bits == 32 {
        (i32::MIN, i32::MAX)
    } else {
        (-(1i32 << (bits - 1)), (1i32 << (bits - 1)) - 1)
    };
    let amp = f64::from(hi);
    match kind % KINDS {
        // Uniform noise over the whole range.
        0 => (0..count).map(|_| r.sample(bits)).collect(),
        // Small values around zero.
        1 => (0..count).map(|_| (r.next() % 7) as i32 - 3).collect(),
        // Only the extremes, at random.
        2 => (0..count)
            .map(|_| if r.next() & 1 == 0 { lo } else { hi })
            .collect(),
        // One constant value.
        3 => {
            let v = r.sample(bits);
            vec![v; count]
        }
        // Extremes alternating from sample to sample, and between channels.
        4 => (0..count)
            .map(|i| {
                if (i / channels + i).is_multiple_of(2) {
                    lo
                } else {
                    hi
                }
            })
            .collect(),
        5 => vec![0; count],
        // The low bits of every sample are zero.
        6 => {
            let shift = (r.next() % u64::from(bits.min(24))) as u32;
            (0..count)
                .map(|_| (r.sample(bits) >> shift) << shift)
                .collect()
        }
        // A tone with noise: what predictors are for.
        7 => {
            let step = 0.01 + (r.next() % 100) as f64 / 500.0;
            (0..count)
                .map(|i| {
                    let t = (i / channels) as f64;
                    let v = (t * step).sin() * amp * 0.7 + (t * step * 0.31).sin() * amp * 0.2;
                    (v as i64 + i64::from(r.sample(bits.min(6))))
                        .clamp(i64::from(lo), i64::from(hi)) as i32
                })
                .collect()
        }
        // Three low sines and a little noise: what high-rate recordings of band-limited audio
        // look like, where the precision of the predictor matters.
        8 => {
            let f = [0.0023, 0.0067, 0.016];
            (0..count)
                .map(|i| {
                    let t = (i / channels) as f64;
                    let v =
                        0.3 * (t * f[0]).sin() + 0.2 * (t * f[1]).sin() + 0.1 * (t * f[2]).sin();
                    ((v * amp) as i64 + i64::from(r.sample(3))).clamp(i64::from(lo), i64::from(hi))
                        as i32
                })
                .collect()
        }
        // Stereo-correlated: the second channel follows the first.
        _ => {
            let base: Vec<i32> = (0..count).map(|_| r.sample(bits.min(20))).collect();
            (0..count)
                .map(|i| {
                    let l = base[i - i % channels];
                    if i % channels == 0 {
                        l
                    } else {
                        l.saturating_add(r.sample(3)).clamp(lo, hi)
                    }
                })
                .collect()
        }
    }
}

/// Every frame within the raw bound, given its own number of sample frames.
pub fn assert_frames_within_bound(e: &Encoded, total_frames: usize) {
    for (i, &(s, t)) in e.frames.iter().enumerate() {
        let n = e.block_size.min(total_frames - i * e.block_size);
        assert!(
            t - s <= raw_bound(e.config, n),
            "frame {i} ({n} samples) is {} bytes, bound {}",
            t - s,
            raw_bound(e.config, n)
        );
    }
}
