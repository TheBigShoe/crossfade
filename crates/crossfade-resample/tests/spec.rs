//! Each preset's stated specification (the README's table), measured with least-squares tone
//! fits: passband ripple up to the passband edge, everything that is not the tone (images,
//! aliases, noise) below the stopband attenuation, tones above the output's Nyquist frequency
//! removed, and the round-trip signal-to-noise ratio.
mod common;

use common::*;
use crossfade_resample::{Quality, Resampler};

const N: usize = 8192;
/// Steady-state margin in output samples for the round trip.
const EDGE: usize = 512;

struct Spec {
    /// Passband edge as a fraction of the lower rate's Nyquist frequency.
    pass: f64,
    ripple_db: f64,
    /// Attenuation from the lower rate's Nyquist frequency up.
    stop_db: f64,
    round_trip_db: f64,
}

fn spec(q: Quality) -> Spec {
    match q {
        Quality::Fast => Spec {
            pass: 0.85,
            ripple_db: 0.001,
            stop_db: 100.0,
            round_trip_db: 130.0,
        },
        Quality::Balanced => Spec {
            pass: 0.91,
            ripple_db: 0.0001,
            stop_db: 120.0,
            round_trip_db: 145.0,
        },
        Quality::Best => Spec {
            pass: 0.935,
            ripple_db: 0.000_001,
            stop_db: 160.0,
            round_trip_db: 200.0,
        },
    }
}

/// Converts a 0.5-amplitude tone; returns the steady output (past the filter's reach from
/// either end) and the output index it starts at.
fn tone(a: u32, b: u32, q: Quality, f: f64) -> (Vec<f64>, usize) {
    let reach = Resampler::new(a, b, 1, q).unwrap().latency() * u64::from(b) / u64::from(a);
    let edge = reach as usize + 16;
    let y = mono(a, b, q, &sine(N, f, a.into(), 0.5));
    (steady(&y, edge).to_vec(), edge)
}

#[test]
fn passband_is_flat_to_the_passband_edge() {
    for q in QUALITIES {
        let s = spec(q);
        for (a, b) in [(44_100, 48_000), (48_000, 44_100), (96_000, 44_100)] {
            let edge = s.pass * f64::from(a.min(b)) / 2.0;
            for k in 0..=10 {
                let f = 20.0 + (edge - 20.0) * f64::from(k) / 10.0;
                let (y, at) = tone(a, b, q, f);
                let (amp, _) = fit(&y, at, f, b.into());
                let dev = db(amp / 0.5);
                assert!(
                    dev.abs() <= s.ripple_db,
                    "{q:?} {a} -> {b}: {f:.1} Hz is {dev:.3e} dB"
                );
            }
        }
    }
}

#[test]
fn balanced_is_flat_within_a_hundredth_of_a_db_to_20_khz() {
    for (a, b) in [(44_100, 48_000), (48_000, 44_100)] {
        for f in [20.0, 1000.0, 10_000.0, 15_000.0, 19_000.0, 20_000.0] {
            let (y, at) = tone(a, b, Quality::Balanced, f);
            let (amp, _) = fit(&y, at, f, b.into());
            assert!(db(amp / 0.5).abs() <= 0.01, "{a} -> {b}: {f} Hz");
        }
    }
}

/// Everything in the output that is not the input tone, for tones across the whole input band
/// (transition band included): images when upsampling, numerical noise either way.
#[test]
fn images_are_below_the_stopband_attenuation() {
    for q in QUALITIES {
        let s = spec(q);
        for (a, b) in [(44_100, 48_000), (44_100, 96_000), (8_000, 44_100)] {
            let nyq = f64::from(a) / 2.0;
            for k in 0..=10 {
                let f = 10.0 + (nyq - 20.0) * f64::from(k) / 10.0;
                let (y, at) = tone(a, b, q, f);
                let (_, residual) = fit(&y, at, f, b.into());
                let level = db(residual / (0.5 / 2f64.sqrt()));
                assert!(
                    level <= -s.stop_db,
                    "{q:?} {a} -> {b}: {f:.1} Hz leaves {level:.1} dB"
                );
            }
        }
    }
}

/// Downsampling: tones in the passband come out clean, tones from the output's Nyquist
/// frequency up do not come out at all (no aliases).
#[test]
fn aliases_are_below_the_stopband_attenuation() {
    for q in QUALITIES {
        let s = spec(q);
        for (a, b) in [(48_000, 44_100), (96_000, 44_100), (192_000, 48_000)] {
            let out_nyq = f64::from(b) / 2.0;
            let in_nyq = f64::from(a) / 2.0;
            let tones = (0..=6)
                .map(|k| 10.0 + (s.pass * out_nyq - 10.0) * f64::from(k) / 6.0)
                .chain(
                    (0..=8).map(|k| out_nyq + 5.0 + (in_nyq - out_nyq - 10.0) * f64::from(k) / 8.0),
                );
            for f in tones {
                let (y, at) = tone(a, b, q, f);
                let level = if f < out_nyq {
                    fit(&y, at, f, b.into()).1
                } else {
                    rms(&y)
                };
                let level = db(level / (0.5 / 2f64.sqrt()));
                assert!(
                    level <= -s.stop_db,
                    "{q:?} {a} -> {b}: {f:.1} Hz leaves {level:.1} dB"
                );
            }
        }
    }
}

/// The milestone's exit criterion: a 1 kHz sine at -1 dBFS through 44.1 kHz -> 48 kHz ->
/// 44.1 kHz, compared sample by sample with the original over the steady part. The default
/// must reach 110 dB and `Best` 140 dB; each preset's stated figure is higher.
#[test]
fn round_trip_44k1_48k_snr_exit_criterion() {
    for q in QUALITIES {
        let x = sine(4 * N, 1000.0, 44_100.0, libm::pow(10.0, -1.0 / 20.0));
        let back = mono(48_000, 44_100, q, &mono(44_100, 48_000, q, &x));
        // Two roundings up can add a frame.
        assert!(back.len() - x.len() <= 1);
        let err: Vec<f64> = steady(&x, 2 * EDGE)
            .iter()
            .zip(steady(&back, 2 * EDGE))
            .map(|(p, r)| p - r)
            .collect();
        let snr = db(rms(steady(&x, 2 * EDGE)) / rms(&err));
        let floor = match q {
            Quality::Fast => 0.0_f64,
            Quality::Balanced => 110.0,
            Quality::Best => 140.0,
        };
        assert!(
            snr >= floor.max(spec(q).round_trip_db),
            "{q:?}: {snr:.1} dB"
        );
    }
}
