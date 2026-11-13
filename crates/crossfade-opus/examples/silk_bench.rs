//! In-process speed of the SILK decoder: decodes `.bit` test vectors (SILK-only ones: 02, 03,
//! 04) on one thread, repeatedly, and prints times real time per vector, best of the runs.
//!
//! ```sh
//! D=.corpus-cache/opus/rfc8251
//! cargo run --release -p crossfade-opus --example silk_bench -- \
//!     $D/testvector02.bit/testvector02.bit $D/testvector03.bit/testvector03.bit \
//!     $D/testvector04.bit/testvector04.bit
//! ```
//!
//! `--runs N` sets the repetitions (default 5); `--seconds S` instead decodes each vector for
//! about S seconds (for an external sampling profiler).

use crossfade_opus::Decoder;
use crossfade_opus::testing::bitfile::read_bit_file;
use std::time::{Duration, Instant};

fn decode_all(packets: &[Vec<u8>]) -> usize {
    let mut d = Decoder::new(2).expect("two channels");
    let mut out = Vec::with_capacity(2880 * 2);
    let mut frames = 0;
    for p in packets {
        out.clear();
        frames += d
            .decode(Some(p), &mut out)
            .expect("a SILK-only vector decodes");
    }
    std::hint::black_box(&out);
    frames
}

fn main() {
    let mut runs = 5;
    let mut seconds: Option<f64> = None;
    let mut files = vec![];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--runs" => runs = args.next().and_then(|v| v.parse().ok()).unwrap_or(runs),
            "--seconds" => seconds = args.next().and_then(|v| v.parse().ok()),
            _ => files.push(a),
        }
    }
    for path in &files {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let packets: Vec<Vec<u8>> = read_bit_file(&bytes)
            .unwrap_or_else(|e| panic!("{path}: {e}"))
            .iter()
            .filter(|p| !p.data.is_empty())
            .map(|p| p.data.to_vec())
            .collect();
        let mut best = Duration::MAX;
        let mut frames;
        let start = Instant::now();
        let mut n = 0;
        loop {
            let t = Instant::now();
            frames = decode_all(&packets);
            best = best.min(t.elapsed());
            n += 1;
            let done = match seconds {
                Some(s) => start.elapsed().as_secs_f64() >= s,
                None => n >= runs,
            };
            if done {
                break;
            }
        }
        let audio = frames as f64 / 48_000.0;
        println!(
            "{path}: {audio:.2} s of audio in {:.1} ms (best of {n}): {:.0}x real time",
            best.as_secs_f64() * 1000.0,
            audio / best.as_secs_f64()
        );
    }
}
