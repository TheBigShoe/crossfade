use super::*;
use crate::packet_duration;

#[test]
fn a_decoder_has_one_or_two_channels() {
    assert!(matches!(Decoder::new(0), Err(Error::BadArgument(_))));
    assert!(matches!(Decoder::new(3), Err(Error::BadArgument(_))));
    for ch in [1, 2] {
        let d = Decoder::new(ch).unwrap();
        assert_eq!(d.channels(), ch);
        assert_eq!(d.final_range(), 0);
    }
}

#[test]
fn an_invalid_packet_is_an_error_and_appends_nothing() {
    let mut d = Decoder::new(2).unwrap();
    let mut out = vec![1.0, 2.0];
    assert!(matches!(
        d.decode(Some(&[]), &mut out),
        Err(Error::InvalidPacket(_))
    ));
    assert!(matches!(
        d.decode(Some(&[(1 << 3) | 1, 0]), &mut out),
        Err(Error::InvalidPacket(_))
    ));
    assert_eq!(out, vec![1.0, 2.0]);
}

/// Deterministic bytes: SILK decodes any, and CELT nearly any.
fn noise(seed: u32, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 24) as u8
        })
        .collect()
}

/// A code 0 packet of `config` with `len` bytes of noise after the TOC.
fn packet(config: u8, stereo: bool, seed: u32, len: usize) -> Vec<u8> {
    let mut p = vec![(config << 3) | (u8::from(stereo) << 2)];
    p.extend(noise(seed, len));
    p
}

/// CELT on noise may code any band energy, so finite is the only bound.
fn bounded(pcm: &[f32]) -> bool {
    pcm.iter().all(|x| x.is_finite())
}

#[test]
fn each_mode_decodes() {
    // A 20 ms CELT FB frame, a 20 ms SILK WB frame, a 20 ms hybrid FB frame.
    for config in [31, 9, 15] {
        let mut d = Decoder::new(2).unwrap();
        let mut out = vec![];
        assert_eq!(
            d.decode(Some(&packet(config, false, 7, 60)), &mut out),
            Ok(960)
        );
        assert_eq!(out.len(), 960 * 2);
        assert!(bounded(&out));
        assert_ne!(d.final_range(), 0, "config {config}");
    }
}

/// One packet of each kind the switches go through: SILK NB/MB/WB 10-60 ms, hybrid SWB/FB
/// 10-20 ms, CELT NB-FB 2.5-20 ms, mono and stereo.
const CONFIGS: [u8; 12] = [0, 3, 5, 11, 12, 15, 16, 19, 23, 26, 28, 31];

#[test]
fn every_mode_switch_decodes_the_packets_duration() {
    for &a in &CONFIGS {
        for &b in &CONFIGS {
            for channels in [1, 2] {
                let mut d = Decoder::new(channels).unwrap();
                let mut out = vec![];
                for (i, &config) in [a, b, a].iter().enumerate() {
                    let p = packet(config, i == 1, u32::from(a) * 64 + u32::from(b), 80);
                    let want = packet_duration(&p).unwrap();
                    out.clear();
                    assert_eq!(d.decode(Some(&p), &mut out), Ok(want), "{a} -> {b}");
                    assert_eq!(out.len(), want * channels);
                    assert!(bounded(&out), "{a} -> {b}");
                }
            }
        }
    }
}

#[test]
fn a_lost_packet_before_any_other_is_nothing() {
    let mut d = Decoder::new(1).unwrap();
    let mut out = vec![];
    assert_eq!(d.decode(None, &mut out), Ok(0));
    assert!(out.is_empty());
}

#[test]
fn an_empty_frame_before_any_audio_is_silence_of_its_duration() {
    // opus_decode_frame: a frame of 0 or 1 bytes is concealed; with no mode yet, as zeros.
    // Its final range is 0.
    for (packet, frames) in [
        (&[(31 << 3)][..], 960),
        (&[(16 << 3) | 0b111, 0], 0),
        (&[(28 << 3) | 0b100, 9], 120),
        (&[(1 << 3) | 2, 0], 1920),
    ] {
        let mut d = Decoder::new(2).unwrap();
        let mut out = vec![];
        let r = d.decode(Some(packet), &mut out);
        if frames == 0 {
            assert!(r.is_err(), "{packet:?}");
            continue;
        }
        assert_eq!(r, Ok(frames), "{packet:?}");
        assert_eq!(out, vec![0.0; frames * 2]);
        assert_eq!(d.final_range(), 0);
    }
}

#[test]
fn reset_forgets_the_stream() {
    let mut d = Decoder::new(2).unwrap();
    let mut out = vec![];
    d.decode(Some(&[31 << 3]), &mut out).unwrap();
    d.reset();
    assert_eq!(d.final_range(), 0);
    out.clear();
    assert_eq!(d.decode(None, &mut out), Ok(0));
}

#[test]
fn a_lost_packet_conceals_the_last_packets_duration() {
    // Code 3 packets of several frames too: the concealment is chunked into 20 ms and less.
    for (config, frames) in [(0, 1), (3, 2), (11, 1), (13, 2), (16, 48), (17, 7), (31, 6)] {
        let mut p = vec![(config << 3) | 3, frames];
        p.extend(noise(u32::from(config), usize::from(frames) * 20));
        let want = packet_duration(&p).unwrap();
        let mut d = Decoder::new(2).unwrap();
        let mut out = vec![];
        assert_eq!(d.decode(Some(&p), &mut out), Ok(want), "config {config}");
        for _ in 0..3 {
            out.clear();
            assert_eq!(d.decode(None, &mut out), Ok(want), "config {config}");
            assert_eq!(out.len(), want * 2);
            assert!(bounded(&out));
            assert_eq!(d.final_range(), 0);
            assert_eq!(d.last_packet_duration(), want);
        }
    }
}

#[test]
fn fec_decodes_the_next_packets_lbrr_in_place_of_the_lost_one() {
    let silk = packet(9, false, 5, 60);
    let celt = packet(31, false, 6, 60);
    let mut d = Decoder::new(1).unwrap();
    let mut out = vec![];
    // Before any packet there is nothing to conceal.
    assert_eq!(d.decode_fec(&silk, &mut out), Ok(0));
    d.decode(Some(&silk), &mut out).unwrap();
    out.clear();
    assert_eq!(d.decode_fec(&silk, &mut out), Ok(960));
    assert_eq!(out.len(), 960);
    assert!(bounded(&out));
    // CELT carries no FEC: the same as a loss.
    let mut lost = d.clone();
    let (mut a, mut b) = (vec![], vec![]);
    assert_eq!(d.decode_fec(&celt, &mut a), Ok(960));
    assert_eq!(lost.decode(None, &mut b), Ok(960));
    assert_eq!(a, b);
    // An invalid packet is refused and appends nothing.
    let before = out.clone();
    assert!(matches!(
        d.decode_fec(&[(1 << 3) | 1, 0], &mut out),
        Err(Error::InvalidPacket(_))
    ));
    assert_eq!(out, before);
}

#[test]
fn the_gain_scales_every_sample() {
    // OPUS_SET_GAIN, Q8 dB: celt_exp2(6.48814081e-4 * gain), the product in float.
    for gain in [-32768, -1536, 256, 1536, 32767] {
        let mut plain = Decoder::new(2).unwrap();
        let mut gained = Decoder::new(2).unwrap();
        gained.set_gain(gain);
        let g = libm::exp(std::f64::consts::LN_2 * f64::from(6.488_141e-4_f32 * f32::from(gain)))
            as f32;
        for seed in 0..3 {
            let p = packet(31, true, seed, 100);
            let (mut a, mut b) = (vec![], vec![]);
            plain.decode(Some(&p), &mut a).unwrap();
            gained.decode(Some(&p), &mut b).unwrap();
            let want: Vec<f32> = a.iter().map(|x| x * g).collect();
            assert_eq!(b, want, "gain {gain}");
        }
        // A reset keeps it, as OPUS_RESET_STATE does.
        gained.reset();
        assert_eq!(gained.gain(), gain);
    }
}
