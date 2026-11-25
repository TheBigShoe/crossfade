use super::*;
use crate::{Decoder, Mode, Packet};

pub(crate) fn config(channels: u8, bitrate: u32, vbr: bool, d: FrameDuration) -> EncoderConfig {
    EncoderConfig {
        channels,
        bitrate,
        vbr,
        frame_duration: d,
        ..EncoderConfig::default()
    }
}

/// `seconds` of test signal `kind`, interleaved, at 48 kHz.
pub(crate) fn signal(kind: &str, channels: usize, seconds: f32) -> Vec<f32> {
    let len = (48_000.0 * seconds) as usize;
    let mut seed = 0x9e37_79b9u32;
    let mut out = Vec::with_capacity(len * channels);
    for i in 0..len {
        let t = i as f32 / 48_000.0;
        for ch in 0..channels {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 8) as f32 / (1 << 24) as f32) * 2.0 - 1.0;
            let tau = 2.0 * std::f32::consts::PI;
            out.push(match kind {
                "silence" => 0.0,
                "tone" => 0.5 * libm::sinf(tau * (440.0 + 110.0 * ch as f32) * t),
                "sweep" => 0.4 * libm::sinf(tau * (50.0 + 9000.0 * t) * t),
                "noise" => 0.3 * noise,
                "clicks" => {
                    if i % 4800 < 48 {
                        0.9 * noise
                    } else {
                        0.0005 * noise
                    }
                }
                "dc" => 0.25,
                "full" => {
                    if (i / 53) % 2 == 0 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                "loud" => 4.0 * libm::sinf(tau * 1000.0 * t),
                "music" => music(t, ch, noise),
                _ => unreachable!("{kind}"),
            });
        }
    }
    out
}

/// A few seconds of something like music at `t`: a chord of harmonic notes panned apart, a
/// kick every half second, a hi-hat on the off-beats, a little hiss.
fn music(t: f32, ch: usize, noise: f32) -> f32 {
    let tau = 2.0 * std::f32::consts::PI;
    let mut v = 0.0;
    for (k, f0) in [220.0f32, 277.18, 329.63].into_iter().enumerate() {
        let pan = [0.8f32, 0.5, 0.2][k];
        let gain = if ch == 0 { pan } else { 1.0 - pan };
        let mut a = 0.12f32;
        for h in 1..=8 {
            v += gain * a * libm::sinf(tau * f0 * h as f32 * t + k as f32);
            a *= 0.6;
        }
    }
    let beat = t % 0.5;
    v += 0.5 * libm::expf(-beat * 30.0) * libm::sinf(tau * 60.0 * beat);
    let off = (t + 0.25) % 0.5;
    v += 0.15 * libm::expf(-off * 120.0) * noise;
    v + 0.003 * noise
}

/// The `opus_compare` quality figure of `got` (decoded, `channels` interleaved, full scale
/// 1.0) against `want`, which it lags by `delay` samples.
pub(crate) fn quality(want: &[f32], got: &[f32], channels: usize, delay: usize) -> f32 {
    let frames = got.len() / channels - delay;
    let mut reference = Vec::with_capacity(2 * frames);
    for i in 0..frames {
        for c in 0..2 {
            reference.push(want[i * channels + c.min(channels - 1)] * 32768.0);
        }
    }
    let decoded: Vec<f32> = got[delay * channels..]
        .iter()
        .map(|v| v * 32768.0)
        .collect();
    crate::testing::compare::compare(&reference, &decoded, channels == 2, 48_000)
        .expect("comparable")
        .q
}

/// Encodes `pcm`, checking that each packet parses, is CELT and decodes with the encoder's
/// final range; returns the packets and the decoded audio.
pub(crate) fn encode_and_decode(cfg: &EncoderConfig, pcm: &[f32]) -> (Vec<Vec<u8>>, Vec<f32>) {
    let channels = usize::from(cfg.channels);
    let mut enc = Encoder::new(cfg).expect("valid config");
    let mut dec = Decoder::new(channels).expect("decoder");
    let n = enc.frame_size();
    let mut packets = vec![];
    let mut out = vec![];
    for (i, frame) in pcm.chunks_exact(n * channels).enumerate() {
        let mut packet = vec![0xAA; 3];
        enc.encode(frame, &mut packet).expect("encodes");
        let parsed = Packet::parse(&packet).expect("parses");
        assert_eq!(parsed.toc().mode(), Mode::Celt);
        assert_eq!(parsed.duration(), n);
        let got = dec.decode(Some(&packet), &mut out).expect("decodes");
        assert_eq!(got, n);
        assert_eq!(
            dec.final_range(),
            enc.final_range(),
            "packet {i} of {} bytes, {cfg:?}",
            packet.len()
        );
        packets.push(packet);
    }
    (packets, out)
}

#[test]
fn the_config_is_checked() {
    for channels in [0, 3] {
        let cfg = config(channels, 64_000, true, FrameDuration::Ms20);
        assert!(matches!(Encoder::new(&cfg), Err(Error::BadArgument(_))));
    }
    let cfg = config(2, 0, true, FrameDuration::Ms20);
    assert!(matches!(Encoder::new(&cfg), Err(Error::BadArgument(_))));
    let d = EncoderConfig::default();
    assert_eq!(d.frame_duration, FrameDuration::Ms20);
    assert!(Encoder::new(&d).is_ok());
}

#[test]
fn frame_size_and_lookahead() {
    for (d, n) in [
        (FrameDuration::Ms2_5, 120),
        (FrameDuration::Ms5, 240),
        (FrameDuration::Ms10, 480),
        (FrameDuration::Ms20, 960),
    ] {
        let enc = Encoder::new(&config(1, 64_000, false, d)).expect("valid");
        assert_eq!(enc.frame_size(), n);
        assert_eq!(enc.lookahead(), 312);
    }
}

#[test]
fn input_of_the_wrong_length_is_refused() {
    let mut enc = Encoder::new(&config(2, 64_000, false, FrameDuration::Ms10)).expect("valid");
    let mut packet = vec![];
    assert!(matches!(
        enc.encode(&[0.0; 480], &mut packet),
        Err(Error::BadArgument(_))
    ));
    assert!(enc.encode(&[0.0; 960], &mut packet).is_ok());
}

#[test]
fn every_packet_decodes_with_the_final_range() {
    for kind in [
        "silence", "tone", "sweep", "noise", "clicks", "dc", "full", "loud",
    ] {
        for channels in [1u8, 2] {
            for d in [
                FrameDuration::Ms2_5,
                FrameDuration::Ms5,
                FrameDuration::Ms10,
                FrameDuration::Ms20,
            ] {
                for bitrate in [6_000, 12_000, 32_000, 96_000, 256_000, 510_000] {
                    for vbr in [false, true] {
                        let pcm = signal(kind, usize::from(channels), 0.12);
                        encode_and_decode(&config(channels, bitrate, vbr, d), &pcm);
                    }
                }
            }
        }
    }
}

#[test]
fn cbr_packets_are_the_size_the_bitrate_implies() {
    for channels in [1u8, 2] {
        for (d, fps) in [
            (FrameDuration::Ms2_5, 400),
            (FrameDuration::Ms5, 200),
            (FrameDuration::Ms10, 100),
            (FrameDuration::Ms20, 50),
        ] {
            for bitrate in [6_000u32, 8_000, 24_000, 64_000, 128_000, 300_000, 510_000] {
                let pcm = signal("sweep", usize::from(channels), 0.1);
                let (packets, _) = encode_and_decode(&config(channels, bitrate, false, d), &pcm);
                // libopus clamps the bitrate to 300 kb/s per channel.
                let rate = bitrate.min(300_000 * u32::from(channels)) as usize;
                let want = ((rate * 12 / 8 + 6 * fps) / (12 * fps)).min(1276);
                for p in &packets {
                    assert_eq!(p.len(), want, "{channels} ch {fps}/s {bitrate}");
                }
            }
        }
    }
}

#[test]
fn non_finite_input_is_coded_as_silence() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30] {
        let cfg = config(2, 64_000, true, FrameDuration::Ms20);
        let mut pcm = signal("tone", 2, 0.2);
        for v in pcm[960 * 2 * 3..960 * 2 * 4].iter_mut().step_by(7) {
            *v = bad;
        }
        let (_, out) = encode_and_decode(&cfg, &pcm);
        assert!(out.iter().all(|v| v.is_finite()), "{bad}");
        // The poisoned frame, as decoded one lookahead later, is quiet.
        let at = 960 * 3 + 312;
        let quiet = out[at * 2 + 400..(at + 960) * 2 - 400]
            .iter()
            .all(|v| v.abs() < 0.05);
        assert!(quiet, "{bad}");
    }
}

#[test]
fn reset_starts_the_stream_afresh() {
    let cfg = config(2, 96_000, true, FrameDuration::Ms10);
    let pcm = signal("sweep", 2, 0.1);
    let mut enc = Encoder::new(&cfg).expect("valid");
    let mut first = vec![];
    let mut p = vec![];
    let step = enc.frame_size() * 2;
    for f in pcm.chunks_exact(step) {
        enc.encode(f, &mut p).expect("encodes");
        first.push(p.clone());
    }
    enc.reset();
    for (f, want) in pcm.chunks_exact(step).zip(&first) {
        enc.encode(f, &mut p).expect("encodes");
        assert_eq!(&p, want);
    }
}

/// The SNR in dB of `got` against `want` through the encoder's 3 Hz DC filter (whose phase
/// shift alone would cap it), which `got` lags by `delay` samples per channel, past the
/// first 100 ms.
fn snr(want: &[f32], got: &[f32], channels: usize, delay: usize) -> f64 {
    let coef = 6.3 * 3.0 / 48_000.0f32;
    let mut m = [0.0f32; 2];
    let filtered: Vec<f32> = want
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let c = i % channels;
            let y = x - m[c];
            m[c] = coef * x + 1e-30 + (1.0 - coef) * m[c];
            y
        })
        .collect();
    let (mut sig, mut err) = (0.0f64, 0.0f64);
    for i in 4800 * channels..got.len() {
        let w = f64::from(filtered[i - delay * channels]);
        sig += w * w;
        err += (w - f64::from(got[i])).powi(2);
    }
    10.0 * libm::log10(sig / err.max(1e-30))
}

#[test]
fn stereo_quality_rises_with_the_bitrate() {
    // The opus_compare figure and the SNR the music signal must beat, at 20 ms: what was
    // measured, less 20 (Q swings) and 1 dB. libopus 1.4 forced to CELT at complexity 9 gives
    // the same within 0.5 dB in CBR (README).
    let pcm = signal("music", 2, 1.0);
    for (vbr, table) in [
        (
            true,
            [
                (32_000, -100.0, 15.0),
                (64_000, -75.0, 18.5),
                (96_000, -50.0, 21.9),
                (128_000, -50.0, 23.6),
                (256_000, 35.0, 27.2),
            ],
        ),
        (
            false,
            [
                (32_000, -750.0, 14.0),
                (64_000, -85.0, 17.3),
                (96_000, -70.0, 20.1),
                (128_000, -75.0, 22.8),
                (256_000, -50.0, 26.6),
            ],
        ),
    ] {
        let mut last = f64::MIN;
        for (bitrate, min_q, min_snr) in table {
            let cfg = config(2, bitrate, vbr, FrameDuration::Ms20);
            let (packets, out) = encode_and_decode(&cfg, &pcm);
            let delay = Encoder::new(&cfg).expect("valid").lookahead() as usize;
            let q = quality(&pcm, &out, 2, delay);
            let s = snr(&pcm, &out, 2, delay);
            let what = format!("{bitrate} vbr {vbr}: Q {q}, {s} dB");
            assert!(q > min_q && s > min_snr, "{what}");
            assert!(s > last, "{what}");
            last = s;
            if vbr {
                // Unconstrained VBR over a second: within 15% (measured: 6 to 10% over).
                let bytes: usize = packets.iter().map(Vec::len).sum();
                let rate = bytes as f64 * 8.0;
                assert!(
                    (rate / f64::from(bitrate) - 1.0).abs() < 0.15,
                    "{what}: {rate} b/s"
                );
            }
        }
    }
}

/// The average rate, in b/s, of `cfg` on `pcm` (`seconds` long).
fn average_rate(cfg: &EncoderConfig, pcm: &[f32], seconds: f64) -> f64 {
    let (packets, _) = encode_and_decode(cfg, pcm);
    packets.iter().map(Vec::len).sum::<usize>() as f64 * 8.0 / seconds
}

#[test]
fn constrained_vbr_holds_the_bitrate_plus_the_toc_byte() {
    // CELT's reservoir keeps its own bytes at the bitrate; the TOC byte comes on top, as in
    // libopus (measured identical: +20% at 16 kb/s in 2.5 ms frames, +2.5% in 20 ms).
    for (channels, kind, bitrate, d, fps) in [
        (1u8, "noise", 48_000u32, FrameDuration::Ms10, 100.0),
        (2, "music", 96_000, FrameDuration::Ms20, 50.0),
        (2, "sweep", 24_000, FrameDuration::Ms5, 200.0),
    ] {
        let mut cfg = config(channels, bitrate, true, d);
        cfg.constrained_vbr = true;
        let pcm = signal(kind, usize::from(channels), 2.0);
        let rate = average_rate(&cfg, &pcm, 2.0);
        let want = f64::from(bitrate) + 8.0 * fps;
        assert!(
            (rate / want - 1.0).abs() < 0.02,
            "{kind}: {rate} for {want}"
        );
    }
}

#[test]
fn unconstrained_vbr_averages_near_the_bitrate() {
    // Open loop: only the signal's demands move it. Measured over 20 s at 20 ms: -11% to
    // +5% on music and noise from 16 to 256 kb/s (a sweep can go 45% over at 32 kb/s, as in
    // libopus).
    for (kind, bitrate) in [("music", 64_000u32), ("noise", 128_000)] {
        let cfg = config(2, bitrate, true, FrameDuration::Ms20);
        let pcm = signal(kind, 2, 2.0);
        let rate = average_rate(&cfg, &pcm, 2.0);
        assert!(
            (rate / f64::from(bitrate) - 1.0).abs() < 0.15,
            "{kind}: {rate}"
        );
    }
}
