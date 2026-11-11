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

/// Ogg FLAC goes through the same threaded decoder: the same WAV at any budget, and a stream
/// cut short fails the same way.
#[test]
fn ogg_flac_decodes_the_same_at_any_budget() {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crossfade-format/tests/fixtures/ogg"
    );
    for name in ["smallblock", "stereo16", "mono24", "noise8"] {
        let ogg = std::fs::read(format!("{dir}/{name}.oga")).unwrap();
        let cut = ogg[..ogg.len() * 2 / 3].to_vec();
        for input in [ogg, cut] {
            let outcome = |threads| {
                let mut io = MemoryIo::new();
                io.insert("in.oga", input.clone());
                let mut j = job("in.oga", "out.wav", None);
                j.outputs[0].audio.compression_level = None;
                let limits = Limits {
                    max_threads: threads,
                    ..Limits::default()
                };
                let r = run(&j, &mut io, &limits, &CancelToken::new(), &mut |_| {});
                (r.map_err(|e| e.to_string()), io.get("out.wav"))
            };
            let one = outcome(1);
            for threads in [2, 3, 8, 0] {
                assert!(outcome(threads) == one, "{name}, {threads} threads");
            }
        }
    }
}
