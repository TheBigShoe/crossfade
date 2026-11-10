//! Input shared by the bench and the profiling example: a synthetic signal encoded once through
//! this crate's encoder, returned as STREAMINFO plus pre-split frames.
use crossfade_flac::streaminfo::StreamInfoBlock;
use crossfade_flac::{Encoder, EncoderConfig};

pub(crate) struct Input {
    pub(crate) name: &'static str,
    pub(crate) pcm_bytes: u64,
    pub(crate) info: StreamInfoBlock,
    pub(crate) frames: Vec<Vec<u8>>,
}

/// A sine mix plus seeded noise, so the residual is realistic.
pub(crate) fn input(name: &'static str, rate: u32, bits: u32, seconds: u32) -> Input {
    let amp = f64::from((1u32 << (bits - 1)) - 1);
    let mut state = 0x9e37_79b9u32;
    let mut samples = Vec::new();
    for t in 0..rate as usize * seconds as usize {
        for ch in 0..2 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(state >> 8) / f64::from(1u32 << 24) - 0.5;
            let x = t as f64;
            let tone = (x * (0.06 + 0.013 * f64::from(ch))).sin() * 0.4
                + (x * 0.0007).sin() * 0.25
                + (x * 0.0213).sin() * 0.1;
            samples.push(((tone + noise * 0.02) * amp) as i32);
        }
    }
    let mut enc = Encoder::new(EncoderConfig::new(rate, 2, bits as u8)).unwrap();
    let (mut bytes, mut ends) = (Vec::new(), Vec::new());
    enc.write_ends(&samples, &mut bytes, &mut ends).unwrap();
    enc.flush_ends(&mut bytes, &mut ends).unwrap();
    let mut start = 0;
    let frames = ends
        .iter()
        .map(|&end| {
            let f = bytes[start..end].to_vec();
            start = end;
            f
        })
        .collect();
    Input {
        name,
        pcm_bytes: (samples.len() * (bits as usize).div_ceil(8)) as u64,
        info: enc.finish(),
        frames,
    }
}
