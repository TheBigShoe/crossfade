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
