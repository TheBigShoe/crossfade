//! Decoder speed with the reader out of the picture: frames are split once in setup. Throughput
//! is the PCM size, so it compares with `flac -d` in MB/s of audio. The end-to-end headline
//! numbers are the `flac_decode` cases in `crossfade-format`'s `audio` bench.
mod support;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use crossfade_core::SampleFormat;
use crossfade_flac::Decoder;
use crossfade_flac::crc::crc16;
use crossfade_flac::md5::Md5;
use std::hint::black_box;
use std::time::Duration;

fn bench(c: &mut Criterion) {
    let long = support::input("60s_44k1_16bit_stereo", 44_100, SampleFormat::S16, 60);
    let wide = support::input("10s_96k_24bit_stereo", 96_000, SampleFormat::S24, 10);

    let mut group = c.benchmark_group("decode_frames");
    group
        .sample_size(20)
        .measurement_time(Duration::from_secs(6));
    for s in [&long, &wide] {
        group.throughput(Throughput::Bytes(s.pcm_bytes));
        group.bench_function(s.name, |b| {
            let mut out = Vec::new();
            b.iter(|| {
                let mut d = Decoder::new(&s.info).unwrap();
                for f in &s.frames {
                    let block = d.decode_frame(f).unwrap();
                    block.interleave_i32(0, &mut out);
                }
                black_box(d.finish().unwrap());
            });
        });
    }
    group.finish();

    let data: Vec<u8> = (0..4 << 20)
        .map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    let mut group = c.benchmark_group("checksums");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("md5", |b| {
        b.iter_batched(
            Md5::new,
            |mut m| {
                m.update(black_box(&data));
                black_box(m.finalize())
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("crc16", |b| b.iter(|| black_box(crc16(black_box(&data)))));
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
