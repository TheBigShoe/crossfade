//! Encoder speed on synthetic audio: 60 s of 44.1 kHz 16-bit stereo and 10 s of 96 kHz 24-bit
//! stereo, a sine mix plus seeded noise (the signals `crossfade-format`'s benches use), at
//! levels 0, 5 and 8, on the calling thread and (`/auto`) with the automatic thread count.
//! Throughput is the PCM size, so the numbers read in MB/s of audio.
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use crossfade_flac::{Encoder, EncoderConfig};
use std::hint::black_box;
use std::time::Duration;

struct Signal {
    name: &'static str,
    config: EncoderConfig,
    samples: Vec<i32>,
}

fn signal(name: &'static str, rate: u32, bits: u8, seconds: u32) -> Signal {
    let amp = f64::from((1u32 << (bits - 1)) - 1);
    let mut state = 0x9e37_79b9u32;
    let mut samples = Vec::with_capacity(rate as usize * seconds as usize * 2);
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
    Signal {
        name,
        config: EncoderConfig::new(rate, 2, bits),
        samples,
    }
}

fn bench(c: &mut Criterion) {
    let signals = [
        signal("stereo_16bit_44k_60s", 44_100, 16, 60),
        signal("stereo_24bit_96k_10s", 96_000, 24, 10),
    ];
    let mut group = c.benchmark_group("flac_encode");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(8));
    for s in &signals {
        let bytes = s.samples.len() * usize::from(s.config.bits_per_sample).div_ceil(8);
        group.throughput(Throughput::Bytes(bytes as u64));
        for (level, threads, suffix) in [0u8, 5, 8]
            .into_iter()
            .flat_map(|l| [(l, 1, ""), (l, EncoderConfig::AUTO_THREADS, "/auto")])
        {
            let config = EncoderConfig {
                level,
                threads,
                ..s.config
            };
            let mut out = Vec::with_capacity(s.samples.len() * 4);
            group.bench_function(format!("{}/level{level}{suffix}", s.name), |b| {
                b.iter(|| {
                    out.clear();
                    let mut enc = Encoder::new(config).unwrap();
                    enc.write(&s.samples, &mut out).unwrap();
                    enc.flush(&mut out).unwrap();
                    black_box((enc.finish(), out.len()))
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
