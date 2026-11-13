//! Times decoding of RFC 8251 test vectors in this process, on one thread: the whole `.bit`
//! file decoded again and again with one `Decoder` (two output channels, as `opus_demo -d
//! 48000 2`), for at least `--secs` seconds (default 3), best of the rounds. Prints times real
//! time (audio seconds per wall second) per vector.
//!
//! ```sh
//! D=.corpus-cache/opus/rfc8251
//! cargo run --release -p crossfade-opus --example celt_bench -- \
//!     $D/testvector01.bit/testvector01.bit $D/testvector07.bit/testvector07.bit \
//!     $D/testvector11.bit/testvector11.bit
//! ```

use crossfade_opus::Decoder;
use crossfade_opus::testing::bitfile::read_bit_file;
use std::time::{Duration, Instant};

fn main() {
    let mut secs = 3.0f64;
    let mut files = vec![];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--secs" {
            secs = args.next().and_then(|s| s.parse().ok()).expect("--secs N");
        } else {
            files.push(a);
        }
    }
    for path in &files {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let packets = read_bit_file(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        let mut out = Vec::with_capacity(1 << 22);
        let mut best = Duration::MAX;
        let mut frames = 0usize;
        let mut rounds = 0;
        let started = Instant::now();
        while rounds < 3 || started.elapsed().as_secs_f64() < secs {
            let mut d = Decoder::new(2).expect("two channels");
            out.clear();
            frames = 0;
            let t = Instant::now();
            for p in &packets {
                frames += d
                    .decode((!p.data.is_empty()).then_some(p.data), &mut out)
                    .unwrap_or_else(|e| panic!("{path}: {e}"));
            }
            best = best.min(t.elapsed());
            rounds += 1;
        }
        let audio = frames as f64 / 48000.0;
        println!(
            "{path}: {audio:.2} s of audio, best of {rounds}: {:.1} ms, {:.0}x real time",
            best.as_secs_f64() * 1e3,
            audio / best.as_secs_f64()
        );
    }
}
