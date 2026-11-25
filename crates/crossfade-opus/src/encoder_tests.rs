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
                _ => unreachable!("{kind}"),
            });
        }
    }
    out
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
