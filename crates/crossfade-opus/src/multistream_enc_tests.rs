use super::*;
use crate::encoder::tests::{config, signal};
use crate::{FrameDuration, MultistreamDecoder, Packet};

/// `seconds` of a different note on each of `channels` channels (so that a channel in the
/// wrong place shows), with a beat of noise, interleaved.
fn surround(channels: usize, seconds: f32) -> Vec<f32> {
    let tau = 2.0 * std::f32::consts::PI;
    let len = (48_000.0 * seconds) as usize;
    let mut seed = 0x1357_9bdfu32;
    let mut out = Vec::with_capacity(len * channels);
    for i in 0..len {
        let t = i as f32 / 48_000.0;
        for ch in 0..channels {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 8) as f32 / (1 << 24) as f32) * 2.0 - 1.0;
            let f = 200.0 * (ch + 1) as f32 + 37.0;
            let beat = libm::expf(-(t % 0.5) * 40.0);
            out.push(0.25 * libm::sinf(tau * f * t) + 0.05 * beat * noise);
        }
    }
    out
}

/// Encodes `pcm` and decodes each packet through the head's mapping, checking every
/// stream's final range; returns the packets and the decoded audio.
fn round_trip(cfg: &EncoderConfig, family: u8, pcm: &[f32]) -> (Vec<Vec<u8>>, Vec<f32>) {
    let mut enc = MultistreamEncoder::new(cfg, family).expect("valid");
    let head = enc.head(48_000);
    let parsed = OpusHead::parse(&head.to_bytes()).expect("a valid head");
    assert_eq!(parsed, head);
    let ch = usize::from(cfg.channels);
    let mut dec = MultistreamDecoder::new(&head.mapping, ch).expect("decoder");
    let n = enc.frame_size();
    let (mut packets, mut out) = (vec![], vec![]);
    for (i, frame) in pcm.chunks_exact(n * ch).enumerate() {
        let mut p = vec![];
        enc.encode(frame, &mut p).expect("encodes");
        assert_eq!(dec.decode(Some(&p), &mut out).expect("decodes"), n);
        assert_eq!(
            dec.final_ranges(),
            enc.final_ranges(),
            "packet {i}, {cfg:?}"
        );
        packets.push(p);
    }
    (packets, out)
}

/// The SNR in dB of channel `c` of `got` against `want` (both `channels` wide), which it
/// lags by `delay`, past the first 100 ms.
fn channel_snr(want: &[f32], got: &[f32], channels: usize, c: usize, delay: usize) -> f64 {
    let (mut sig, mut err) = (0.0f64, 0.0f64);
    for i in 4800 + delay..got.len() / channels {
        let w = f64::from(want[(i - delay) * channels + c]);
        sig += w * w;
        err += (w - f64::from(got[i * channels + c])).powi(2);
    }
    10.0 * libm::log10(sig / err.max(1e-30))
}

#[test]
fn the_head_says_what_the_streams_are() {
    // libopus's vorbis_mappings: (streams, coupled, table) per channel count.
    let want: [(u8, u8, &[u8]); 8] = [
        (1, 0, &[0]),
        (1, 1, &[0, 1]),
        (2, 1, &[0, 2, 1]),
        (2, 2, &[0, 1, 2, 3]),
        (3, 2, &[0, 4, 1, 2, 3]),
        (4, 2, &[0, 4, 1, 2, 3, 5]),
        (4, 3, &[0, 4, 1, 2, 3, 5, 6]),
        (5, 3, &[0, 6, 1, 2, 3, 4, 5, 7]),
    ];
    for (k, (streams, coupled, table)) in want.into_iter().enumerate() {
        let channels = k as u8 + 1;
        let enc = MultistreamEncoder::new(&config(channels, 64_000, true, FrameDuration::Ms20), 1)
            .expect("valid");
        let head = enc.head(44_100);
        assert_eq!(head.channels, channels);
        assert_eq!(head.pre_skip, 312);
        assert_eq!(head.input_sample_rate, 44_100);
        assert_eq!(head.output_gain, 0);
        let m = &head.mapping;
        assert_eq!(
            (m.family, m.streams, m.coupled, m.table.as_slice()),
            (1, streams, coupled, table)
        );
    }
    for channels in [1u8, 2] {
        let enc = MultistreamEncoder::new(&config(channels, 64_000, true, FrameDuration::Ms20), 0)
            .expect("valid");
        let bytes = enc.head(48_000).to_bytes();
        assert_eq!(bytes.len(), 19);
        assert_eq!(bytes[18], 0);
    }
    let cfg = |ch| config(ch, 64_000, true, FrameDuration::Ms20);
    assert!(MultistreamEncoder::new(&cfg(3), 0).is_err());
    assert!(MultistreamEncoder::new(&cfg(9), 1).is_err());
    assert!(MultistreamEncoder::new(&cfg(0), 1).is_err());
    assert!(MultistreamEncoder::new(&cfg(2), 255).is_err());
}

#[test]
fn the_mono_and_stereo_encoder_writes_its_head() {
    for channels in [1u8, 2] {
        let enc = Encoder::new(&config(channels, 64_000, true, FrameDuration::Ms20)).expect("ok");
        let head = enc.head(96_000);
        let bytes = head.to_bytes();
        assert_eq!(&bytes[..8], b"OpusHead");
        assert_eq!(OpusHead::parse(&bytes).expect("valid"), head);
        assert_eq!(
            (head.channels, head.pre_skip, head.input_sample_rate),
            (channels, 312, 96_000)
        );
        assert_eq!(head.mapping.family, 0);
    }
}

#[test]
fn every_channel_count_decodes_with_the_final_ranges() {
    for channels in 1u8..=8 {
        for (bitrate, vbr) in [
            (24_000 * u32::from(channels), true),
            (12_000 * u32::from(channels), false),
            (96_000 * u32::from(channels), false),
        ] {
            for d in [
                FrameDuration::Ms10,
                FrameDuration::Ms20,
                FrameDuration::Ms60,
            ] {
                let cfg = config(channels, bitrate, vbr, d);
                let pcm = surround(usize::from(channels), 0.12);
                let (_, out) = round_trip(&cfg, 1, &pcm);
                assert!(out.iter().all(|v| v.is_finite()));
            }
        }
    }
}

#[test]
fn each_channel_comes_out_where_it_went_in() {
    for channels in [3usize, 5, 6, 8] {
        let cfg = config(
            channels as u8,
            96_000 * channels as u32,
            true,
            FrameDuration::Ms20,
        );
        let pcm = surround(channels, 1.0);
        let (_, out) = round_trip(&cfg, 1, &pcm);
        // The LFE, last from 6 channels on, keeps no 1 kHz tone.
        let lfe = channels >= 6;
        for c in 0..channels {
            if lfe && c == channels - 1 {
                continue;
            }
            let s = channel_snr(&pcm, &out, channels, c, 312);
            assert!(s > 10.0, "{channels} ch, channel {c}: {s} dB");
        }
    }
}

#[test]
fn the_surround_streams_are_coded_as_libopus_codes_them() {
    // 5.1: two coupled streams (front, rear) forced to stereo CELT, the centre mono, the LFE
    // CELT narrowband.
    let cfg = config(6, 256_000, true, FrameDuration::Ms20);
    let pcm = surround(6, 0.5);
    let (packets, _) = round_trip(&cfg, 1, &pcm);
    for p in &packets {
        let mut rest = p.as_slice();
        for s in 0..4 {
            let parsed = if s < 3 {
                Packet::parse_self_delimited(rest).expect("parses")
            } else {
                Packet::parse(rest).expect("parses")
            };
            let toc = parsed.toc();
            match s {
                0 | 1 => assert_eq!((toc.mode(), toc.stereo()), (Mode::Celt, true)),
                3 => assert_eq!(
                    (toc.mode(), toc.bandwidth()),
                    (Mode::Celt, Bandwidth::Narrow)
                ),
                _ => assert!(!toc.stereo()),
            }
            rest = &rest[parsed.byte_len()..];
        }
        assert!(rest.is_empty());
    }
}

#[test]
fn cbr_totals_are_exact_and_vbr_totals_near_the_bitrate() {
    for channels in [3u8, 6, 8] {
        for bitrate in [64_000u32, 192_000, 448_000] {
            let pcm = surround(usize::from(channels), 1.0);
            let cbr = config(channels, bitrate, false, FrameDuration::Ms20);
            let (packets, _) = round_trip(&cbr, 1, &pcm);
            let want = (3 * bitrate / (3 * 8 * 50)) as usize;
            for p in &packets {
                assert_eq!(p.len(), want, "{channels} ch {bitrate}");
            }
            let vbr = config(channels, bitrate, true, FrameDuration::Ms20);
            let (packets, _) = round_trip(&vbr, 1, &pcm);
            let rate = packets.iter().map(Vec::len).sum::<usize>() as f64 * 8.0;
            assert!(
                // Measured: -7% (8 ch at 64 kb/s) to +33% (5.1 at 64 kb/s, CELT's open-loop
                // VBR on the rear pair's noise bursts); +2% to +15% from 192 kb/s.
                (rate / f64::from(bitrate) - 1.0).abs() < 0.4,
                "{channels} ch {bitrate}: {rate}"
            );
        }
    }
}

#[test]
fn surround_quality_rises_with_the_bitrate() {
    // The mean SNR over 5.1's five full-range channels: measured 23.8, 27.2 and 29.9 dB,
    // less 1 dB.
    let pcm = surround(6, 1.0);
    let mut last = f64::MIN;
    for (bitrate, min) in [(128_000u32, 22.8), (256_000, 26.2), (448_000, 28.9)] {
        let cfg = config(6, bitrate, true, FrameDuration::Ms20);
        let (_, out) = round_trip(&cfg, 1, &pcm);
        let mean = [0, 1, 2, 4, 5]
            .iter()
            .map(|&c| channel_snr(&pcm, &out, 6, c, 312))
            .sum::<f64>()
            / 5.0;
        assert!(mean > min && mean > last, "{bitrate}: {mean} dB");
        last = mean;
    }
}

#[test]
fn family_0_is_the_plain_encoder_framed_alike() {
    for channels in [1u8, 2] {
        let cfg = config(channels, 48_000, true, FrameDuration::Ms20);
        let pcm = signal("music", usize::from(channels), 0.2);
        let (packets, _) = round_trip(&cfg, 0, &pcm);
        let mut plain = Encoder::new(&cfg).expect("valid");
        let mut p = vec![];
        for (f, want) in pcm.chunks_exact(960 * usize::from(channels)).zip(&packets) {
            plain.encode(f, &mut p).expect("encodes");
            assert_eq!(&p, want);
        }
    }
}

#[test]
fn reset_starts_the_streams_afresh() {
    let cfg = config(6, 256_000, true, FrameDuration::Ms20);
    let pcm = surround(6, 0.2);
    let mut enc = MultistreamEncoder::new(&cfg, 1).expect("valid");
    let mut first = vec![];
    let mut p = vec![];
    for f in pcm.as_chunks::<5760>().0 {
        enc.encode(f, &mut p).expect("encodes");
        first.push(p.clone());
    }
    enc.reset();
    for (f, want) in pcm.as_chunks::<5760>().0.iter().zip(&first) {
        enc.encode(f, &mut p).expect("encodes");
        assert_eq!(&p, want);
    }
}

#[test]
fn surround_gives_the_same_bytes_on_every_platform() {
    // Frozen: a change here changes the encoder's output, which must be on purpose.
    let mut cases = vec![];
    for (channels, bitrate, vbr) in [
        (6u8, 256_000u32, true),
        (6, 160_000, false),
        (8, 320_000, true),
    ] {
        let mut enc =
            MultistreamEncoder::new(&config(channels, bitrate, vbr, FrameDuration::Ms20), 1)
                .expect("valid");
        let pcm = surround(usize::from(channels), 0.4);
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut total = 0;
        let mut p = vec![];
        for f in pcm.chunks_exact(960 * usize::from(channels)) {
            enc.encode(f, &mut p).expect("encodes");
            total += p.len();
            for b in p
                .iter()
                .copied()
                .chain(enc.final_ranges().iter().flat_map(|r| r.to_le_bytes()))
            {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
        cases.push((total, h));
    }
    assert_eq!(
        cases,
        [
            (13912, 12513354539588341673),
            (8000, 1453688392283971629),
            (17779, 16932559918747612100)
        ]
    );
}
