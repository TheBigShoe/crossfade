//! How far a decoder reset mid-stream (a seek) is from one that decoded from the start: for
//! each RFC 8251 vector, targets spread over the stream, and pre-rolls of 80 ms, 200 ms and
//! 1 s, the decoder is reset at the last packet starting at least the pre-roll before the
//! target and run on; the largest |difference| from the continuous decode at or after the
//! target is printed (and the 16-bit steps it is).
//!
//! ```sh
//! D=.corpus-cache/opus/rfc8251
//! cargo run --release -p crossfade-opus --example opus_seek_convergence -- \
//!     $D/testvector*.bit/testvector*.bit
//! ```

use crossfade_opus::Decoder;
use crossfade_opus::testing::bitfile::read_bit_file;

fn main() {
    let prerolls = [3840usize, 9600, 48_000];
    for path in std::env::args().skip(1) {
        let bit = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let packets = read_bit_file(&bit).unwrap();
        let mut d = Decoder::new(2).unwrap();
        let mut whole = vec![];
        let mut starts = vec![];
        for p in &packets {
            starts.push(whole.len() / 2);
            d.decode((!p.data.is_empty()).then_some(p.data), &mut whole)
                .unwrap();
        }
        let total = whole.len() / 2;
        let name = path.rsplit('/').next().unwrap_or(&path);
        let mut line = format!("{name}:");
        for &pre in &prerolls {
            let mut worst = 0.0f32;
            for k in 1..20 {
                let target = total * k / 20;
                let Some(first) = starts.iter().rposition(|&s| s + pre <= target) else {
                    continue;
                };
                let mut d = Decoder::new(2).unwrap();
                let mut out = vec![];
                let at = starts[first];
                // Decode on to 0.5 s past the target.
                for p in &packets[first..] {
                    if at + out.len() / 2 > target + 24_000 {
                        break;
                    }
                    d.decode((!p.data.is_empty()).then_some(p.data), &mut out)
                        .unwrap();
                }
                let from = (target - at) * 2;
                let n = out.len().min(whole.len() - at * 2);
                for i in from..n {
                    worst = worst.max((out[i] - whole[at * 2 + i]).abs());
                }
            }
            line.push_str(&format!(
                "  {} ms: {:.3e} ({:.1} steps)",
                pre / 48,
                worst,
                worst * 32768.0
            ));
        }
        println!("{line}");
    }
}
