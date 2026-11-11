//! `measure_loudness`: one decode pass through the meter.
mod common;

use common::{job, spec, wav_bytes};
use crossfade_core::{ChannelLayout, Error, Limits, SampleFormat};
use crossfade_graph::{CancelToken, measure_loudness, run};
use crossfade_testkit::memory::MemoryIo;

/// `seconds` of a 1 kHz sine at `dbfs` peak on the channels in `on`, as 16-bit PCM.
fn tone_wav(rate: u32, layout: ChannelLayout, on: &[usize], dbfs: f64, seconds: f64) -> Vec<u8> {
    let ch = usize::from(layout.channels());
    let amp = 10f64.powf(dbfs / 20.0) * 32767.0;
    let n = (f64::from(rate) * seconds) as usize;
    let mut pcm = Vec::with_capacity(n * ch * 2);
    for i in 0..n {
        let s = (amp * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / f64::from(rate)).sin())
            .round() as i16;
        for c in 0..ch {
            pcm.extend_from_slice(&(if on.contains(&c) { s } else { 0 }).to_le_bytes());
        }
    }
    wav_bytes(&spec(rate, SampleFormat::S16, layout), &pcm)
}

fn measure(io: &mut MemoryIo, path: &str) -> Result<crossfade_graph::Measurements, Error> {
    measure_loudness(path, io, &Limits::default(), &CancelToken::new())
}

fn io_with(path: &str, wav: Vec<u8>) -> MemoryIo {
    let mut io = MemoryIo::new();
    io.insert(path, wav);
    io
}

#[test]
fn ebu_3341_case_1_reads_minus_23_lufs() {
    let mut io = io_with(
        "a.wav",
        tone_wav(48_000, ChannelLayout::stereo(), &[0, 1], -23.0, 20.0),
    );
    let m = measure(&mut io, "a.wav").unwrap();
    assert!(
        (m.integrated.unwrap() + 23.0).abs() < 0.1,
        "{:?}",
        m.integrated
    );
    assert_eq!(m.frames, 960_000);
    assert!(m.true_peak.unwrap() >= m.sample_peak);
}

#[test]
fn the_same_signal_through_flac_measures_the_same_bits() {
    let mut io = io_with(
        "a.wav",
        tone_wav(48_000, ChannelLayout::stereo(), &[0, 1], -23.0, 20.0),
    );
    run(
        &job("a.wav", "a.flac", None),
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(
        measure(&mut io, "a.flac").unwrap(),
        measure(&mut io, "a.wav").unwrap()
    );
}

#[test]
fn silence_and_short_programmes_have_no_loudness() {
    let mut io = io_with(
        "s.wav",
        tone_wav(48_000, ChannelLayout::stereo(), &[], -23.0, 3.0),
    );
    let m = measure(&mut io, "s.wav").unwrap();
    assert_eq!(
        (m.integrated, m.loudness_range, m.sample_peak),
        (None, None, 0.0)
    );
    let mut io = io_with(
        "t.wav",
        tone_wav(48_000, ChannelLayout::stereo(), &[0, 1], -10.0, 0.2),
    );
    assert_eq!(measure(&mut io, "t.wav").unwrap().integrated, None);
}

#[test]
fn mono_and_the_extreme_rates_measure() {
    for (rate, layout) in [
        (8_000, ChannelLayout::mono()),
        (192_000, ChannelLayout::stereo()),
    ] {
        let mut io = io_with("a.wav", tone_wav(rate, layout, &[0], -20.0, 5.0));
        let m = measure(&mut io, "a.wav").unwrap();
        assert!(m.integrated.is_some(), "{rate}");
    }
}

#[test]
fn a_5_1_file_leaves_the_lfe_out_of_loudness_but_not_the_peaks() {
    let mut io = io_with(
        "a.wav",
        tone_wav(
            48_000,
            ChannelLayout::default_for(6).unwrap(),
            &[3],
            -6.0,
            5.0,
        ),
    );
    let m = measure(&mut io, "a.wav").unwrap();
    assert_eq!(m.integrated, None);
    assert!(m.sample_peak > 0.49);
}

#[test]
fn a_rate_below_8_khz_is_unsupported_and_named() {
    let mut io = io_with(
        "a.wav",
        tone_wav(4_000, ChannelLayout::stereo(), &[0, 1], -20.0, 1.0),
    );
    let Err(Error::Unsupported { what }) = measure(&mut io, "a.wav") else {
        panic!("expected Unsupported")
    };
    assert!(what.contains("4000"), "{what}");
}

#[test]
fn a_cancelled_pass_stops() {
    let mut io = io_with(
        "a.wav",
        tone_wav(48_000, ChannelLayout::stereo(), &[0, 1], -23.0, 2.0),
    );
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        measure_loudness("a.wav", &mut io, &Limits::default(), &cancel).err(),
        Some(Error::Cancelled)
    );
}
