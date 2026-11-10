//! Deterministic test signals, bit-identical on every OS.

use crossfade_core::AudioSpec;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    Sine { hz: f64, amplitude: f64 },
    /// Linear in frequency over the rendered length.
    Sweep {
        from_hz: f64,
        to_hz: f64,
        amplitude: f64,
    },
    Impulse { at_frame: usize, amplitude: f64 },
    Silence,
    /// Uniform white noise from a fixed PRNG.
    Noise { seed: u64, amplitude: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignalError {
    /// Frequency is negative, not finite, or above half the sample rate.
    BadFrequency { hz: f64, nyquist: f64 },
}

impl fmt::Display for SignalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadFrequency { hz, nyquist } => {
                write!(f, "frequency {hz} Hz outside 0..={nyquist} Hz")
            }
        }
    }
}

impl std::error::Error for SignalError {}

pub fn render(
    _signal: &Signal,
    _spec: &AudioSpec,
    _frames: usize,
) -> Result<Vec<f64>, SignalError> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{ChannelLayout, SampleFormat};

    fn spec(rate: u32, channels: u16) -> AudioSpec {
        AudioSpec::new(
            rate,
            SampleFormat::F32,
            ChannelLayout::unpositioned(channels).unwrap(),
        )
        .unwrap()
    }

    /// Mono zero crossings (sign changes) per second of `x` at `rate`.
    fn crossing_hz(x: &[f64], rate: u32) -> f64 {
        let n = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        n as f64 * f64::from(rate) / x.len() as f64
    }

    fn sine(hz: f64, amplitude: f64) -> Signal {
        Signal::Sine { hz, amplitude }
    }

    #[test]
    fn length_and_channels_identical() {
        let out = render(&sine(440.0, 0.5), &spec(48_000, 3), 100).unwrap();
        assert_eq!(out.len(), 300);
        for f in out.chunks(3) {
            assert_eq!(f[0].to_bits(), f[1].to_bits());
            assert_eq!(f[0].to_bits(), f[2].to_bits());
        }
    }

    #[test]
    fn sine_measures_one_khz_with_exact_peak() {
        let out = render(&sine(1000.0, 0.7), &spec(48_000, 1), 48_000).unwrap();
        let hz = crossing_hz(&out, 48_000);
        assert!((hz - 1000.0).abs() < 1.5, "{hz}");
        let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert!((peak - 0.7).abs() < 1e-9, "{peak}");
    }

    #[test]
    fn sine_first_samples_are_pinned_bit_patterns() {
        let out = render(&sine(1000.0, 0.5), &spec(48_000, 1), 5).unwrap();
        let bits: Vec<u64> = out.iter().map(|v| v.to_bits()).collect();
        assert_eq!(
            bits,
            [
                0x0,
                0x3fb0b5150f6da2d0,
                0x3fc0907dc1930690,
                0x3fc87de2a6aea963,
                0x3fcfffffffffffff
            ]
        );
    }

    #[test]
    fn silence_is_exactly_zero() {
        let out = render(&Signal::Silence, &spec(8000, 2), 64).unwrap();
        assert_eq!(out.len(), 128);
        assert!(out.iter().all(|v| v.to_bits() == 0));
    }

    #[test]
    fn impulse_has_one_nonzero_frame() {
        let sig = Signal::Impulse {
            at_frame: 7,
            amplitude: 0.25,
        };
        let out = render(&sig, &spec(8000, 2), 20).unwrap();
        for (i, f) in out.chunks(2).enumerate() {
            let want = if i == 7 { 0.25 } else { 0.0 };
            assert_eq!(f, [want, want], "frame {i}");
        }
    }

    #[test]
    fn impulse_beyond_the_end_renders_silence() {
        let sig = Signal::Impulse {
            at_frame: 99,
            amplitude: 1.0,
        };
        let out = render(&sig, &spec(8000, 1), 10).unwrap();
        assert!(out.iter().all(|v| *v == 0.0));
    }

    fn noise(seed: u64, amplitude: f64) -> Signal {
        Signal::Noise { seed, amplitude }
    }

    #[test]
    fn noise_first_samples_are_pinned() {
        let out = render(&noise(12345, 1.0), &spec(48_000, 1), 4).unwrap();
        assert_eq!(
            out,
            [
                -0.7338406626771454,
                -0.5903667332766818,
                -0.7609148339817691,
                -0.6477643855100776
            ]
        );
    }

    #[test]
    fn noise_seeds_decide_the_samples() {
        let s = spec(48_000, 1);
        let a = render(&noise(1, 1.0), &s, 1000).unwrap();
        assert_eq!(a, render(&noise(1, 1.0), &s, 1000).unwrap());
        assert_ne!(a, render(&noise(2, 1.0), &s, 1000).unwrap());
    }

    #[test]
    fn noise_is_bounded_and_centred() {
        let out = render(&noise(99, 0.5), &spec(48_000, 1), 100_000).unwrap();
        assert!(out.iter().all(|v| v.abs() <= 0.5));
        let mean = out.iter().sum::<f64>() / out.len() as f64;
        assert!(mean.abs() < 0.01, "{mean}");
    }

    #[test]
    fn sweep_start_and_end_rates_match() {
        let sig = Signal::Sweep {
            from_hz: 500.0,
            to_hz: 4000.0,
            amplitude: 1.0,
        };
        let rate = 48_000;
        let out = render(&sig, &spec(rate, 1), 4 * 48_000).unwrap();
        let tenth = out.len() / 10;
        let first = crossing_hz(&out[..tenth], rate);
        let last = crossing_hz(&out[out.len() - tenth..], rate);
        // Averages over the window, so allow the in-window sweep spread.
        assert!((first - 500.0).abs() / 500.0 < 0.1, "{first}");
        assert!((last - 4000.0).abs() / 4000.0 < 0.1, "{last}");
        assert!(last > 6.0 * first);
    }

    #[test]
    fn frequency_above_nyquist_is_rejected() {
        let s = spec(48_000, 1);
        assert!(render(&sine(24_001.0, 1.0), &s, 10).is_err());
        assert!(render(&sine(f64::NAN, 1.0), &s, 10).is_err());
        assert!(render(&sine(-1.0, 1.0), &s, 10).is_err());
        let sweep = Signal::Sweep {
            from_hz: 100.0,
            to_hz: 30_000.0,
            amplitude: 1.0,
        };
        assert!(render(&sweep, &s, 10).is_err());
        assert!(render(&sine(24_000.0, 1.0), &s, 10).is_ok());
    }
}
