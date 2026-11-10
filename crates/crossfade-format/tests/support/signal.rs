//! The samples behind the AIFF fixtures. The example that writes the source WAVs and the test
//! that checks the decoded fixtures both include this file, so they cannot drift apart.

use crossfade_core::{AudioSpec, ChannelLayout, SampleFormat};
use crossfade_testkit::{
    pcm::encode_le,
    signal::{Signal, render},
};

pub(crate) const RATE: u32 = 22_050;
pub(crate) const FRAMES: usize = 600;

/// Interleaved little-endian bytes of two different sines, in `format`'s own layout (8-bit is
/// offset binary, as WAV stores it).
pub(crate) fn le_bytes(format: SampleFormat) -> Vec<u8> {
    let mono = AudioSpec::new(RATE, format, ChannelLayout::mono()).unwrap();
    let left = render(
        &Signal::Sine {
            hz: 440.0,
            amplitude: 0.6,
        },
        &mono,
        FRAMES,
    )
    .unwrap();
    let right = render(
        &Signal::Sine {
            hz: 1_000.0,
            amplitude: 0.3,
        },
        &mono,
        FRAMES,
    )
    .unwrap();
    let both: Vec<f64> = left
        .iter()
        .zip(&right)
        .flat_map(|(l, r)| [*l, *r])
        .collect();
    encode_le(&both, format)
}
