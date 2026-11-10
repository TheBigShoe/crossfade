//! Reader and codec speed on synthetic audio: FLAC decode and encode (the project's FLAC speed
//! baseline) and AIFF and WAV read+decode for comparison. Inputs are built here (a sine mix
//! plus seeded noise, so the FLAC residual is realistic) and encoded once, in setup. Throughput
//! is the PCM size, so every group compares in MB/s of audio.
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use crossfade_codec::{EncodeOptions, decoder_for, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Limits, MediaSink, Packet, SampleFormat,
    Samples, StreamInfo, Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use std::hint::black_box;
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Signal {
    name: &'static str,
    spec: AudioSpec,
    /// Interleaved sample values, sign-extended.
    samples: Vec<i32>,
}

impl Signal {
    fn pcm_bytes(&self) -> u64 {
        (self.samples.len() * self.spec.format().bytes_per_sample()) as u64
    }

    /// Signed little-endian PCM, for timing another tool on the same audio.
    fn raw(&self) -> Vec<u8> {
        let width = self.spec.format().bytes_per_sample();
        self.samples
            .iter()
            .flat_map(|s| s.to_le_bytes()[..width].to_vec())
            .collect()
    }

    fn frame(&self) -> AudioFrame {
        let mut f = AudioFrame::new(&self.spec);
        f.samples = match self.spec.format() {
            SampleFormat::S16 => Samples::S16(self.samples.iter().map(|&v| v as i16).collect()),
            _ => Samples::S24(self.samples.clone()),
        };
        f
    }
}

fn signal(name: &'static str, rate: u32, format: SampleFormat, seconds: u32) -> Signal {
    let bits = format.bits() as u32;
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
        spec: AudioSpec::new(rate, format, ChannelLayout::stereo()).unwrap(),
        samples,
    }
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

impl MediaSink for Shared {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        MediaSink::write_all(&mut *self.0.lock().unwrap(), buf)
    }

    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        MediaSink::seek(&mut *self.0.lock().unwrap(), pos)
    }

    fn finish(&mut self) -> std::io::Result<()> {
        MediaSink::finish(&mut *self.0.lock().unwrap())
    }
}

fn encode_packets(s: &Signal, codec: CodecId, level: Option<u8>) -> (Vec<u8>, Vec<Packet>) {
    let mut enc = encoder_for(
        codec,
        &s.spec,
        &EncodeOptions {
            compression_level: level,
        },
    )
    .unwrap();
    let mut packets = Vec::new();
    let mut p = Packet::default();
    enc.send_frame(&s.frame()).unwrap();
    enc.drain();
    while enc.receive_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    (enc.extradata().to_vec(), packets)
}

/// A finished file in memory, written through the real encoder and muxer.
fn file(s: &Signal, format: FormatId, codec: CodecId) -> Vec<u8> {
    let (extradata, packets) = encode_packets(s, codec, None);
    let mut info = StreamInfo::new(
        0,
        codec,
        s.spec.clone(),
        TimeBase::new(1, i64::from(s.spec.sample_rate())).unwrap(),
    );
    info.extradata = extradata.clone();
    let sink = Shared::default();
    let mut mux = create_muxer(format, Box::new(sink.clone()), &info, &Tags::new()).unwrap();
    for p in &packets {
        mux.write_packet(p).unwrap();
    }
    mux.set_extradata(&extradata).unwrap();
    mux.finish().unwrap();
    let bytes = sink.0.lock().unwrap().get_ref().clone();
    bytes
}

/// Reader plus decoder over memory, to the end of the stream.
fn read_decode(bytes: Vec<u8>) -> u64 {
    let (_, mut demux) = open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    let stream = demux.streams()[0].clone();
    let mut decoder = decoder_for(&stream).unwrap();
    let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
    let mut frames = 0u64;
    while demux.read_packet(&mut packet).unwrap() {
        decoder.send_packet(&packet).unwrap();
        while decoder.receive_frame(&mut frame).unwrap() {
            frames += frame.frames() as u64;
        }
    }
    decoder.drain();
    while decoder.receive_frame(&mut frame).unwrap() {}
    frames
}

fn bench(c: &mut Criterion) {
    let long = signal("60s_44k1_16bit_stereo", 44_100, SampleFormat::S16, 60);
    let wide = signal("10s_96k_24bit_stereo", 96_000, SampleFormat::S24, 10);
    // `CROSSFADE_BENCH_DUMP=dir` writes the inputs as raw PCM so `flac` can be timed on them.
    if let Some(dir) = std::env::var_os("CROSSFADE_BENCH_DUMP") {
        for s in [&long, &wide] {
            std::fs::write(
                std::path::Path::new(&dir).join(format!("{}.raw", s.name)),
                s.raw(),
            )
            .unwrap();
        }
    }

    let mut group = c.benchmark_group("flac_decode");
    group
        .sample_size(20)
        .measurement_time(Duration::from_secs(8));
    for s in [&long, &wide] {
        let flac = file(s, FormatId::Flac, CodecId::Flac);
        group.throughput(Throughput::Bytes(s.pcm_bytes()));
        group.bench_function(s.name, |b| {
            b.iter_batched(
                || flac.clone(),
                |bytes| black_box(read_decode(bytes)),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();

    let mut group = c.benchmark_group("flac_encode");
    group
        .sample_size(10)
        .measurement_time(Duration::from_secs(8));
    for s in [&long, &wide] {
        group.throughput(Throughput::Bytes(s.pcm_bytes()));
        for level in [0u8, 5, 8] {
            let frame = s.frame();
            group.bench_function(format!("{}/level{level}", s.name), |b| {
                b.iter(|| {
                    let mut enc = encoder_for(
                        CodecId::Flac,
                        &s.spec,
                        &EncodeOptions {
                            compression_level: Some(level),
                        },
                    )
                    .unwrap();
                    let mut p = Packet::default();
                    enc.send_frame(&frame).unwrap();
                    enc.drain();
                    let mut bytes = 0;
                    while enc.receive_packet(&mut p).unwrap() {
                        bytes += p.data.len();
                    }
                    black_box(bytes)
                });
            });
        }
    }
    group.finish();

    let mut group = c.benchmark_group("pcm_read_decode");
    group
        .sample_size(20)
        .measurement_time(Duration::from_secs(5));
    group.throughput(Throughput::Bytes(long.pcm_bytes()));
    for (name, format, codec) in [
        ("aiff", FormatId::Aiff, CodecId::PcmS16Be),
        ("wav", FormatId::Wav, CodecId::PcmS16Le),
    ] {
        let bytes = file(&long, format, codec);
        group.bench_function(format!("{name}/{}", long.name), |b| {
            b.iter_batched(
                || bytes.clone(),
                |bytes| black_box(read_decode(bytes)),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
