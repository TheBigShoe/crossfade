//! Meter speed on 60 s of stereo 48 kHz. Throughput is frames, so the report reads as
//! multiples of real time: frames per second divided by 48,000.
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use crossfade_loudness::{ChannelRole, Meter, Options};
use std::hint::black_box;
use std::time::Duration;

const RATE: u32 = 48_000;
const SECONDS: usize = 60;

fn programme() -> Vec<f64> {
    let mut state = 12345u64;
    (0..RATE as usize * SECONDS * 2)
        .map(|i| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let noise = ((state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0;
            noise * [0.5, 0.05][(i / 2 / RATE as usize / 5) % 2]
        })
        .collect()
}

fn bench(c: &mut Criterion) {
    let samples = programme();
    let roles = [ChannelRole::Left, ChannelRole::Right];
    let mut group = c.benchmark_group("meter_60s_stereo_48k");
    group
        .sample_size(20)
        .measurement_time(Duration::from_secs(5))
        .throughput(Throughput::Elements((RATE as usize * SECONDS) as u64));
    for (name, options) in [
        ("all_on", Options::default()),
        (
            "integrated_only",
            Options {
                true_peak: false,
                loudness_range: false,
            },
        ),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| {
                let mut m = Meter::new(RATE, &roles, options).unwrap();
                m.push(&samples);
                black_box(m.finish())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
