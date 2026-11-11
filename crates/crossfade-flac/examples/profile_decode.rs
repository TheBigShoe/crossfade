//! A looping decode for a sampling profiler: `cargo build --release -p crossfade-flac
//! --example profile_decode`, then `sample <pid> 10` on the running binary.
#[allow(dead_code)]
#[path = "../benches/support/mod.rs"]
mod support;

use crossfade_flac::Decoder;

fn main() {
    let s = support::input("60s_44k1_16bit_stereo", 44_100, 16, 60);
    let mut out = Vec::new();
    loop {
        let mut d = Decoder::new(&s.info).unwrap();
        for f in &s.frames {
            d.decode_frame(f).unwrap().interleave_i16(0, &mut out);
        }
        d.finish().unwrap();
    }
}
