//! Signal generators shared by the integration tests. Everything is synthesised here; no test
//! reads a file. Test maths may use `std`: only the crate's own code is held to `libm`.
#![allow(dead_code, unreachable_pub)]

use crossfade_loudness::{ChannelRole, Meter, Options};

pub const FS: u32 = 48_000;

pub fn lin(dbfs: f64) -> f64 {
    10f64.powf(dbfs / 20.0)
}

pub fn db(lin: f64) -> f64 {
    20.0 * lin.log10()
}

pub fn frames(rate: u32, ms: u32) -> usize {
    (u64::from(rate) * u64::from(ms) / 1000) as usize
}

pub const STEREO: [ChannelRole; 2] = [ChannelRole::Left, ChannelRole::Right];

pub fn loudness_only() -> Options {
    Options {
        true_peak: false,
        loudness_range: false,
    }
}

pub fn meter(rate: u32, roles: &[ChannelRole]) -> Meter {
    Meter::new(rate, roles, loudness_only()).unwrap()
}

/// Interleaved sine, each channel at its own linear amplitude, starting at phase 0.
pub fn tone(rate: u32, amps: &[f64], hz: f64, ms: u32) -> Vec<f64> {
    let n = frames(rate, ms);
    let mut out = Vec::with_capacity(n * amps.len());
    for i in 0..n {
        let s = (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin();
        out.extend(amps.iter().map(|a| a * s));
    }
    out
}

/// The same level on `channels` channels, in dBFS peak.
pub fn tone_db(rate: u32, channels: usize, dbfs: f64, ms: u32) -> Vec<f64> {
    tone(rate, &vec![lin(dbfs); channels], 1000.0, ms)
}

pub fn silence(rate: u32, channels: usize, ms: u32) -> Vec<f64> {
    vec![0.0; frames(rate, ms) * channels]
}

/// A 1 kHz stereo sequence of (milliseconds, dBFS) steps.
pub fn steps(rate: u32, seq: &[(u32, f64)]) -> Vec<f64> {
    seq.iter()
        .flat_map(|&(ms, db)| tone_db(rate, 2, db, ms))
        .collect()
}

pub fn near(got: Option<f64>, want: f64, tol: f64, what: &str) {
    let got = got.unwrap_or_else(|| panic!("{what}: no value"));
    eprintln!("{what}: {got:.3} (want {want})");
    assert!(
        (got - want).abs() <= tol,
        "{what}: {got} vs {want} (+-{tol})"
    );
}

pub fn peak_meter(rate: u32, roles: &[ChannelRole]) -> Meter {
    Meter::new(
        rate,
        roles,
        Options {
            true_peak: true,
            loudness_range: false,
        },
    )
    .unwrap()
}

/// Stereo sine at `hz` with a linear fade of `fade_ms` at both ends.
pub fn faded_sine(rate: u32, hz: f64, amp: f64, phase_deg: f64, ms: u32, fade_ms: u32) -> Vec<f64> {
    let n = frames(rate, ms);
    let fade = frames(rate, fade_ms).max(1);
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let ph =
            phase_deg.to_radians() + 2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate);
        let env = ((i + 1).min(n - i).min(fade)) as f64 / fade as f64;
        let s = amp * env * ph.sin();
        out.extend([s, s]);
    }
    out
}
