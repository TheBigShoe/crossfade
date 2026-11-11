//! Measures each preset's specification as the tests do and prints the README's table:
//! `cargo run --release -p crossfade-resample --example spec`.
#[path = "../tests/common/mod.rs"]
mod common;

use common::*;
use crossfade_resample::{Quality, Resampler};

const N: usize = 1 << 15;

/// Worst passband deviation (dB) over tones up to `edge` Hz.
fn ripple(a: u32, b: u32, q: Quality, edge: f64) -> f64 {
    let mut worst: f64 = 0.0;
    for k in 1..=40 {
        let f = edge * f64::from(k) / 40.0;
        let y = mono(a, b, q, &sine(N, f, a.into(), 0.5));
        let (amp, _) = fit(steady(&y, 2000), 2000, f, b.into());
        worst = worst.max(db(amp / 0.5).abs());
    }
    worst
}

/// Worst level (dB re the input tone) of everything but the tone, over tones from 10 Hz to
/// just below `top` Hz: images when upsampling, aliases and noise either way. Tones at or
/// above the output's Nyquist frequency must vanish entirely.
fn spurious(a: u32, b: u32, q: Quality, top: f64) -> f64 {
    let mut worst = f64::NEG_INFINITY;
    for k in 0..60 {
        let f = 10.0 + (top - 20.0) * f64::from(k) / 59.0;
        let y = mono(a, b, q, &sine(N, f, a.into(), 0.5));
        let s = steady(&y, 2000);
        let level = if f < f64::from(b) / 2.0 {
            fit(s, 2000, f, b.into()).1
        } else {
            rms(s)
        };
        worst = worst.max(db(level / (0.5 / 2f64.sqrt())));
    }
    worst
}

/// Round-trip SNR (dB) of a 1 kHz sine at -1 dBFS through `a -> b -> a`, against the original.
fn round_trip(a: u32, b: u32, q: Quality) -> f64 {
    let x = sine(N, 1000.0, a.into(), libm::pow(10.0, -1.0 / 20.0));
    let back = mono(b, a, q, &mono(a, b, q, &x));
    let err: Vec<f64> = steady(&x, 4000)
        .iter()
        .zip(steady(&back, 4000))
        .map(|(p, q)| p - q)
        .collect();
    db(rms(steady(&x, 4000)) / rms(&err))
}

/// The largest |sample| a full-scale square wave at `f` Hz becomes.
fn overshoot(a: u32, b: u32, q: Quality, f: f64) -> f64 {
    let period = f64::from(a) / f;
    let x: Vec<f64> = (0..N)
        .map(|t| {
            if (t as f64 / period).fract() < 0.5 {
                1.0
            } else {
                -1.0
            }
        })
        .collect();
    mono(a, b, q, &x).iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// Worst level (dB) of everything but the tone near the stopband edge, over common pairs.
fn stop_edge(q: Quality) -> f64 {
    let mut worst = f64::NEG_INFINITY;
    for (a, b) in [
        (44_100u32, 48_000u32),
        (48_000, 44_100),
        (96_000, 44_100),
        (44_100, 96_000),
        (192_000, 48_000),
        (8_000, 44_100),
        (48_000, 8_000),
    ] {
        let low = f64::from(a.min(b)) / 2.0;
        let edge = (Resampler::new(a, b, 1, q).unwrap().latency() as f64 * f64::from(b)
            / f64::from(a)) as usize
            + 64;
        for k in 0..40 {
            // From just inside the transition band to 5% past the stopband edge.
            let f = low * (0.985 + 0.065 * f64::from(k) / 39.0);
            if f >= f64::from(a) / 2.0 {
                continue;
            }
            let y = mono(a, b, q, &sine(N, f, a.into(), 0.5));
            let s = steady(&y, edge);
            let level = if f < f64::from(b) / 2.0 {
                fit(s, edge, f, b.into()).1
            } else {
                rms(s)
            };
            worst = worst.max(db(level / (0.5 / 2f64.sqrt())));
        }
    }
    worst
}

/// The worst-case gain from |input| <= 1 to |output|: the largest sum of |coefficient| over
/// the taps one output uses. Impulses at `down` consecutive inputs, grouped by output phase,
/// visit every coefficient of every phase once.
fn l1_bound(a: u32, b: u32, q: Quality) -> f64 {
    let g = (1..=a.min(b))
        .rev()
        .find(|d| a % d == 0 && b % d == 0)
        .unwrap();
    let (up, down) = ((b / g) as usize, (a / g) as usize);
    let k = 3000;
    let mut sums = vec![0.0; up];
    for r in 0..down {
        let mut x = vec![0.0; 2 * k];
        x[k + r] = 1.0;
        for (n, v) in mono(a, b, q, &x).iter().enumerate() {
            sums[(n * down) % up] += v.abs();
        }
    }
    sums.into_iter().fold(0.0, f64::max)
}

fn main() {
    for q in QUALITIES {
        let pass = match q {
            Quality::Fast => 0.85,
            Quality::Balanced => 0.91,
            Quality::Best => 0.935,
        } * 22_050.0;
        println!(
            "  ripple to the passband edge {pass:.0} Hz: up {:.2e} dB, down {:.2e} dB",
            ripple(44_100, 48_000, q, pass),
            ripple(48_000, 44_100, q, pass)
        );
        let worst = [
            (44_100, 48_000),
            (48_000, 44_100),
            (44_100, 96_000),
            (96_000, 44_100),
        ]
        .iter()
        .flat_map(|&(a, b)| [997.0, 1000.0, 3001.0, 5512.5].map(|f| overshoot(a, b, q, f)))
        .fold(0.0, f64::max);
        println!("  square-wave peak {worst:.4} ({:.2} dB)", db(worst));
        let bound = [
            (44_100, 48_000),
            (48_000, 44_100),
            (44_100, 96_000),
            (96_000, 44_100),
        ]
        .iter()
        .map(|&(a, b)| l1_bound(a, b, q))
        .fold(0.0, f64::max);
        println!(
            "  worst-case gain (L1 of a row) {bound:.4} ({:.2} dB)",
            db(bound)
        );
        println!("  worst near the stopband edge {:.1} dB", stop_edge(q));
        let r = Resampler::new(44_100, 48_000, 1, q).unwrap();
        println!("{q:?}: latency {} frames at 44.1 -> 48", r.latency());
        println!(
            "  ripple to 20 kHz: up {:.2e} dB, down {:.2e} dB",
            ripple(44_100, 48_000, q, 20_000.0),
            ripple(48_000, 44_100, q, 20_000.0)
        );
        println!(
            "  spurious: 44.1->48 {:.1} dB, 48->44.1 {:.1} dB, 96->44.1 {:.1} dB",
            spurious(44_100, 48_000, q, 22_050.0),
            spurious(48_000, 44_100, q, 24_000.0),
            spurious(96_000, 44_100, q, 48_000.0)
        );
        println!(
            "  round trip 1 kHz: 44.1<->48 {:.1} dB, 44.1<->96 {:.1} dB",
            round_trip(44_100, 48_000, q),
            round_trip(44_100, 96_000, q)
        );
    }
}
