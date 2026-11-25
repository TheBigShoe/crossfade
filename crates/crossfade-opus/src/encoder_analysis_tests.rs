//! The decisions the tonality analysis drives, as in libopus: speech or music behind
//! `Signal::Auto`, the bandwidth of a band-limited input, and the VBR target of tonal frames.

use super::modes_tests::{speech, steady};
use super::tests::{config, encode_and_decode, signal};
use super::*;

/// `seconds` of a chord of three harmonic notes, nothing above 2.7 kHz.
fn chord(seconds: f32) -> Vec<f32> {
    let len = (48_000.0 * seconds) as usize;
    let tau = 2.0 * std::f32::consts::PI;
    (0..len)
        .map(|i| {
            let t = i as f32 / 48_000.0;
            let mut v = 0.0;
            for (k, f0) in [220.0f32, 277.18, 329.63].into_iter().enumerate() {
                let mut a = 0.1f32;
                for h in 1..=8 {
                    v += a * libm::sinf(tau * f0 * h as f32 * t + k as f32);
                    a *= 0.6;
                }
            }
            v
        })
        .collect()
}

#[test]
fn auto_codes_music_as_celt_and_speech_as_silk_or_hybrid() {
    // Without the analysis both took the same mode at a rate; with it, Auto is libopus's
    // speech/music decision.
    for (channels, bitrate) in [(1u8, 16_000u32), (1, 24_000), (2, 24_000), (2, 32_000)] {
        let ch = usize::from(channels);
        let cfg = config(channels, bitrate, true, FrameDuration::Ms20);
        let (music_mode, _) = steady(&cfg, &signal("music", ch, 3.0));
        let (speech_mode, _) = steady(&cfg, &speech(ch, 3.0));
        assert_eq!(music_mode, Mode::Celt, "music, {channels} ch at {bitrate}");
        assert_ne!(
            speech_mode,
            Mode::Celt,
            "speech, {channels} ch at {bitrate}"
        );
    }
}

#[test]
fn the_voice_estimate_follows_the_analysis() {
    let estimate = |pcm: &[f32]| {
        let cfg = config(1, 32_000, true, FrameDuration::Ms20);
        let mut enc = Encoder::new(&cfg).expect("valid");
        let mut packet = vec![];
        let mut sum = 0;
        let frames: Vec<&[f32]> = pcm.chunks_exact(960).collect();
        for (i, f) in frames.iter().enumerate() {
            enc.encode(f, &mut packet).expect("encodes");
            if i >= frames.len() / 2 {
                sum += enc.last_voice_est;
            }
        }
        sum / (frames.len() - frames.len() / 2) as i32
    };
    let music = estimate(&signal("music", 1, 3.0));
    let voice = estimate(&speech(1, 3.0));
    // Q7: 0 is music, 115 the most an audio encoder believes in speech.
    assert!(music < 20, "music {music}");
    assert!(voice > 80, "speech {voice}");
}

/// `seconds` of tones every 50 Hz from 100 Hz to below `top`, about -70 dBFS.
fn quiet_tones(top: u32, seconds: f32) -> Vec<f32> {
    let len = (48_000.0 * seconds) as u32;
    let tau = 2.0 * std::f32::consts::PI;
    let freqs: Vec<u32> = (100..top).step_by(50).collect();
    let gain = 0.0004 / (freqs.len() as f32).sqrt();
    (0..len)
        .map(|i| {
            let mut v = 0.0;
            for (k, &f) in freqs.iter().enumerate() {
                // The phase exactly, whatever the time.
                let phase = (f * i % 48_000) as f32 / 48_000.0;
                v += libm::sinf(tau * phase + 0.7 * (k * k) as f32);
            }
            gain * v
        })
        .collect()
}

#[test]
fn the_detected_bandwidth_narrows_a_band_limited_input() {
    // As in libopus with float input (24 bits), the detector sees a band empty only when
    // its high-pass leak is under the noise floor: quiet input. Then a CELT frame at a low
    // rate codes no more than narrowband.
    let mut cfg = config(1, 16_000, true, FrameDuration::Ms20);
    cfg.signal = Signal::Music;
    assert_eq!(
        steady(&cfg, &quiet_tones(3000, 2.0)),
        (Mode::Celt, Bandwidth::Narrow)
    );
    assert_eq!(
        steady(&cfg, &quiet_tones(20_000, 2.0)),
        (Mode::Celt, Bandwidth::Full)
    );
}

fn rate(cfg: &EncoderConfig, pcm: &[f32]) -> f64 {
    let (packets, _) = encode_and_decode(cfg, pcm);
    let seconds = packets.len() as f64 * cfg.frame_duration.samples() as f64 / 48_000.0;
    packets.iter().map(Vec::len).sum::<usize>() as f64 * 8.0 / seconds
}

#[test]
fn tonal_frames_get_more_bits_and_noise_fewer() {
    // The analysis raises the VBR target by the tonality (and lowers it for inactive
    // frames): a steady chord takes more than the bitrate, noise less.
    let mut cfg = config(1, 48_000, true, FrameDuration::Ms20);
    cfg.signal = Signal::Music;
    let tonal = rate(&cfg, &chord(3.0));
    let noise = rate(&cfg, &signal("noise", 1, 3.0));
    assert!(tonal > 1.15 * 48_000.0, "chord {tonal}");
    assert!(noise < 48_000.0, "noise {noise}");
}
