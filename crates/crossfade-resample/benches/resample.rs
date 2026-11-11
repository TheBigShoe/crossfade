//! Conversion speed, one thread: 10 s of stereo noise-plus-tones at each rate pair and
//! quality. Throughput is input frames, so `elem/s` divided by the input rate reads as times
//! real time. `examples/throughput.rs` gives the best of N runs over 60 s for the README.
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use crossfade_resample::{Quality, Resampler};
use std::hint::black_box;
use std::time::Duration;

const PAIRS: [(u32, u32); 5] = [
    (44_100, 48_000),
    (48_000, 44_100),
    (96_000, 44_100),
    (44_100, 96_000),
    (192_000, 48_000),
];

fn signal(rate: u32, seconds: u32) -> Vec<f64> {
    let mut state = 0x9e37_79b9u32;
    let frames = (rate * seconds) as usize;
    let mut x = Vec::with_capacity(frames * 2);
    for t in 0..frames {
        for ch in 0..2 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(state >> 8) / f64::from(1u32 << 24) - 0.5;
            let p = t as f64 * (0.06 + 0.013 * f64::from(ch));
            x.push(0.4 * libm::sin(p) + 0.1 * libm::sin(p * 0.37) + 0.02 * noise);
        }
    }
    x
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("resample");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(4));
    for (a, b) in PAIRS {
        let x = signal(a, 10);
        group.throughput(Throughput::Elements(u64::from(a) * 10));
        for q in [Quality::Fast, Quality::Balanced, Quality::Best] {
            let mut r = Resampler::new(a, b, 2, q).unwrap();
            let mut out = Vec::with_capacity(x.len() * 5);
            group.bench_function(format!("{a}_{b}/{q:?}"), |bench| {
                bench.iter(|| {
                    out.clear();
                    for chunk in x.chunks(2 * 4096) {
                        r.process(chunk, &mut out).unwrap();
                    }
                    r.flush(&mut out);
                    black_box(out.len())
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
