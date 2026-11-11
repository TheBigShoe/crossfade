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
