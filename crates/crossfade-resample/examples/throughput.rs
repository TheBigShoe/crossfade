//! Best-of-N speed for the README: 60 s of stereo through each pair and quality, one thread,
//! in 4096-frame chunks. `cargo run --release -p crossfade-resample --example throughput [N]`.
use crossfade_resample::{Quality, Resampler};
use std::time::Instant;

fn main() {
    let runs: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);
    println!("| pair | Fast | Balanced | Best |\n|---|---|---|---|");
    for (a, b) in [
        (44_100u32, 48_000u32),
        (48_000, 44_100),
        (96_000, 44_100),
        (44_100, 96_000),
        (192_000, 48_000),
        (44_100, 44_101),
    ] {
        let mut state = 1u64;
        let x: Vec<f64> = (0..a as usize * 60 * 2)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5
            })
            .collect();
        let mut row = format!("| {} -> {} kHz |", f64::from(a) / 1e3, f64::from(b) / 1e3);
        for q in [Quality::Fast, Quality::Balanced, Quality::Best] {
            let mut r = Resampler::new(a, b, 2, q).unwrap();
            let mut out = Vec::with_capacity(x.len() * 3);
            let mut best = f64::INFINITY;
            for _ in 0..runs {
                out.clear();
                let start = Instant::now();
                for chunk in x.chunks(2 * 4096) {
                    r.process(chunk, &mut out).unwrap();
                }
                r.flush(&mut out);
                best = best.min(start.elapsed().as_secs_f64());
            }
            row += &format!(" {:.0} ms, {:.0}x |", best * 1e3, 60.0 / best);
        }
        println!("{row}");
    }
}
