//! The thread budget in `Limits` is an execution setting: the job does not carry it and the
//! output does not depend on it.
mod common;

use common::{job, sine_wav, spec};
use crossfade_core::{ChannelLayout, Limits, SampleFormat};
use crossfade_graph::{CancelToken, run};
use crossfade_testkit::memory::MemoryIo;

fn convert(wav: &[u8], level: Option<u8>, max_threads: usize) -> Vec<u8> {
    let mut io = MemoryIo::new();
    io.insert("in.wav", wav.to_vec());
    let mut j = job("in.wav", "out.flac", None);
    j.outputs[0].audio.compression_level = level;
    let limits = Limits {
        max_threads,
        ..Limits::default()
    };
    run(&j, &mut io, &limits, &CancelToken::new(), &mut |_| {}).unwrap();
    io.get("out.flac").unwrap()
}

#[test]
fn the_thread_budget_changes_no_output_byte() {
    for (format, layout, level) in [
        (SampleFormat::S16, ChannelLayout::stereo(), Some(0)),
        (SampleFormat::S24, ChannelLayout::mono(), None),
        (SampleFormat::S16, ChannelLayout::stereo(), Some(8)),
    ] {
        let (_, wav) = sine_wav(&spec(44_100, format, layout), 4096 * 9 + 123);
        let one = convert(&wav, level, 1);
        for threads in [2, 8, 0] {
            assert!(
                convert(&wav, level, threads) == one,
                "{format:?} level {level:?}, {threads} threads"
            );
        }
    }
}

#[test]
fn the_default_budget_is_the_whole_machine() {
    assert_eq!(Limits::default().max_threads, 0);
}

fn run_job(io: &mut MemoryIo, input: &str, output: &str, level: Option<u8>, max_threads: usize) {
    let mut j = job(input, output, None);
    j.outputs[0].audio.compression_level = level;
    let limits = Limits {
        max_threads,
        ..Limits::default()
    };
    run(&j, io, &limits, &CancelToken::new(), &mut |_| {}).unwrap();
}

/// FLAC in: decoding to WAV and re-encoding to FLAC give the same bytes at any budget.
#[test]
fn decoding_and_reencoding_flac_change_no_byte_at_any_budget() {
    let (_, wav) = sine_wav(
        &spec(44_100, SampleFormat::S16, ChannelLayout::stereo()),
        4096 * 12 + 5,
    );
    let flac = convert(&wav, Some(0), 1);
    let outputs = |threads| {
        let mut io = MemoryIo::new();
        io.insert("in.flac", flac.clone());
        run_job(&mut io, "in.flac", "out.wav", None, threads);
        run_job(&mut io, "in.flac", "again.flac", Some(5), threads);
        (io.get("out.wav").unwrap(), io.get("again.flac").unwrap())
    };
    let one = outputs(1);
    assert_eq!(&one.0[44..], &wav[44..]);
    for threads in [2, 3, 8, 0] {
        assert!(outputs(threads) == one, "{threads} threads");
    }
}
