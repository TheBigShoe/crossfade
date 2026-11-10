mod common;

use common::{data_chunk, job, sine_wav, spec};
use crossfade_core::{AudioSpec, ChannelLayout, JobIo, Limits, SampleFormat};
use crossfade_format::wav::FRAMES_PER_PACKET;
use crossfade_graph::{Pipeline, Step};
use crossfade_testkit::{memory::MemoryIo, pcm};

/// Drives `step` by hand to the end and returns every step taken.
fn drive(io: &mut MemoryIo, input: &str, output: &str, fmt: Option<SampleFormat>) -> Vec<Step> {
    let mut p = Pipeline::open(&job(input, output, fmt), io, &Limits::default()).unwrap();
    let mut steps = Vec::new();
    loop {
        let s = p.step().unwrap();
        steps.push(s);
        if s == Step::Finished {
            break;
        }
    }
    io.commit(output).unwrap();
    steps
}

fn stereo(format: SampleFormat) -> AudioSpec {
    spec(8000, format, ChannelLayout::stereo())
}

fn converted(from: SampleFormat, to: Option<SampleFormat>, frames: usize) -> (Vec<u8>, Vec<u8>) {
    let s = stereo(from);
    let (_, wav) = sine_wav(&s, frames);
    let mut io = MemoryIo::new();
    io.insert("in.wav", wav.clone());
    drive(&mut io, "in.wav", "out.wav", to);
    (wav, io.get("out.wav").unwrap())
}

fn oracle(input_pcm: &[u8], from: SampleFormat, to: SampleFormat) -> Vec<u8> {
    pcm::encode_le(&pcm::decode_le(input_pcm, from).unwrap(), to)
}

#[test]
fn s16_to_s24_matches_the_oracle() {
    // More than one packet, so the buffers get reused.
    let (wav, out) = converted(SampleFormat::S16, Some(SampleFormat::S24), 10_000);
    let expected = oracle(data_chunk(&wav), SampleFormat::S16, SampleFormat::S24);
    assert_eq!(data_chunk(&out), &expected[..]);
}

#[test]
fn no_sample_format_keeps_the_payload_byte_identical() {
    let (wav, out) = converted(SampleFormat::S16, None, 10_000);
    assert_eq!(data_chunk(&out), data_chunk(&wav));
}

#[test]
fn s16_to_s24_and_back_is_bit_exact() {
    let (wav, out) = converted(SampleFormat::S16, Some(SampleFormat::S24), 5_000);
    let mut io = MemoryIo::new();
    io.insert("a.wav", out);
    drive(&mut io, "a.wav", "b.wav", Some(SampleFormat::S16));
    assert_eq!(data_chunk(&io.get("b.wav").unwrap()), data_chunk(&wav));
}

#[test]
fn float_targets_and_sources_follow_the_oracle() {
    for (from, to) in [
        (SampleFormat::S16, SampleFormat::F32),
        (SampleFormat::S16, SampleFormat::F64),
        (SampleFormat::F32, SampleFormat::S16),
        (SampleFormat::F32, SampleFormat::S24),
        (SampleFormat::U8, SampleFormat::S32),
    ] {
        let (wav, out) = converted(from, Some(to), 3_000);
        let expected = oracle(data_chunk(&wav), from, to);
        assert_eq!(data_chunk(&out), &expected[..], "{from:?} -> {to:?}");
    }
}

#[test]
fn one_step_per_packet_then_finished_forever() {
    let s = spec(8000, SampleFormat::S16, ChannelLayout::mono());
    let mut io = MemoryIo::new();
    io.insert("in.wav", sine_wav(&s, 10_000).1);
    let mut p = Pipeline::open(
        &job("in.wav", "out.wav", Some(SampleFormat::S24)),
        &mut io,
        &Limits::default(),
    )
    .unwrap();
    let packets = 10_000usize.div_ceil(FRAMES_PER_PACKET);
    let mut done = Vec::new();
    for _ in 0..packets {
        match p.step().unwrap() {
            Step::Progress {
                frames_done,
                frames_total,
            } => {
                assert_eq!(frames_total, Some(10_000));
                done.push(frames_done);
            }
            Step::Finished => panic!("finished early"),
        }
    }
    assert!(done.windows(2).all(|w| w[0] < w[1]), "{done:?}");
    assert_eq!(done.last(), Some(&10_000));
    for _ in 0..3 {
        assert_eq!(p.step().unwrap(), Step::Finished);
    }
}

#[test]
fn empty_input_finishes_on_the_first_step() {
    let (_, out) = converted(SampleFormat::S16, Some(SampleFormat::S24), 0);
    assert!(data_chunk(&out).is_empty());
}
