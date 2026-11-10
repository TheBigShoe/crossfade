//! 60 s of 48 kHz stereo s16 converted to s24 through `run`, all in memory.
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use crossfade_core::{AudioSpec, ChannelLayout, Limits, SampleFormat};
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, run};
use crossfade_testkit::memory::MemoryIo;
use std::hint::black_box;

const SECONDS: usize = 60;
const RATE: u32 = 48_000;

fn wav() -> Vec<u8> {
    let spec = AudioSpec::new(RATE, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    let data_len = SECONDS * RATE as usize * spec.bytes_per_frame();
    // Cheap deterministic ramp; content does not affect the conversion cost.
    let pcm: Vec<u8> = (0..data_len).map(|i| (i % 251) as u8).collect();
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&u32::try_from(36 + data_len).unwrap().to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&RATE.to_le_bytes());
    v.extend_from_slice(&(RATE * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&u32::try_from(data_len).unwrap().to_le_bytes());
    v.extend_from_slice(&pcm);
    v
}

fn bench(c: &mut Criterion) {
    let input = wav();
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput {
            path: "in.wav".into(),
        }],
        outputs: vec![JobOutput {
            path: "out.wav".into(),
            audio: AudioSettings {
                sample_format: Some(SampleFormat::S24),
            },
        }],
    };
    let mut group = c.benchmark_group("convert");
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_function("60s_48k_stereo_s16_to_s24", |b| {
        b.iter(|| {
            let mut io = MemoryIo::new();
            io.insert("in.wav", input.clone());
            run(
                &job,
                &mut io,
                &Limits::default(),
                &CancelToken::new(),
                &mut |_| {},
            )
            .unwrap();
            black_box(io.get("out.wav"))
        });
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
