//! Decodes RFC 8251 test vectors and says how far each gets: per packet (with `--packets`) the
//! TOC, the samples and whether the final range matches the encoder's; per vector the first
//! failure, or Q against `.dec` and `m.dec` (`opus_compare`: pass at Q >= 0).
//!
//! ```sh
//! D=.corpus-cache/opus/rfc8251
//! # Every vector that is CELT only (01, 07, 11), each packet listed:
//! cargo run --release -p crossfade-opus --example opus_vector -- --mode celt --packets \
//!     $D/testvector*.bit/testvector*.bit
//! ```
//!
//! `--mode silk|celt|hybrid` skips vectors with packets of any other mode. The references are
//! found beside each `.bit` file, by replacing `.bit` with `.dec` and `m.dec` in its path (the
//! corpus cache keeps `testvector01.bit/testvector01.bit` and `testvector01.dec/...`).

use crossfade_opus::testing::bitfile::read_pcm16;
use crossfade_opus::testing::compare::compare_pcm16;
use crossfade_opus::testing::vectors::{decode_vector, modes};

fn main() {
    let mut packets = false;
    let mut mode: Option<String> = None;
    let mut files = vec![];
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--packets" => packets = true,
            "--mode" => mode = args.next(),
            _ => files.push(a),
        }
    }
    let mut failed = 0;
    for bit_path in &files {
        let bit = std::fs::read(bit_path).unwrap_or_else(|e| panic!("{bit_path}: {e}"));
        let used = modes(&bit).unwrap_or_else(|e| panic!("{bit_path}: {e}"));
        if let Some(m) = &mode
            && used.iter().any(|u| u != m)
        {
            continue;
        }
        println!(
            "{bit_path} ({})",
            used.into_iter().collect::<Vec<_>>().join(", ")
        );
        let pcm = decode_vector(&bit, 2, |line| {
            if packets {
                println!("  {line}");
            }
        });
        let pcm = match pcm {
            Ok(pcm) => pcm,
            Err(e) => {
                println!("  FAILS: {e}");
                failed += 1;
                continue;
            }
        };
        let mut passes = false;
        for suffix in [".dec", "m.dec"] {
            let path = bit_path.replace(".bit", suffix);
            let Ok(reference) = std::fs::read(&path) else {
                println!("  no {path}");
                continue;
            };
            match compare_pcm16(&read_pcm16(&reference), &pcm, true, 48000) {
                Ok(q) => {
                    passes |= q.passes();
                    println!("  Q {:.1} (err {:.6}) against {path}", q.q, q.err);
                }
                Err(e) => println!("  {e} against {path}"),
            }
        }
        if !passes {
            failed += 1;
        }
        println!("  {}", if passes { "PASSES" } else { "FAILS" });
    }
    std::process::exit(i32::from(failed > 0));
}
