//! Frames through the adapter and back through the decoder adapter.

use super::*;
use crate::alac::decoder;
use crossfade_core::{CodecId, SampleFormat::*, StreamInfo, TimeBase};

fn spec(format: SampleFormat, channels: u8) -> AudioSpec {
    let mask = crossfade_alac::wave_layout(channels).unwrap().mask;
    let layout = ChannelLayout::from_mask(mask, u16::from(channels)).unwrap();
    AudioSpec::new(44100, format, layout).unwrap()
}

/// Deterministic noise on a ramp that stays inside `depth` bits, left-justified into `width`.
fn signal(frames: usize, channels: usize, depth: u32, width: u32) -> Vec<i32> {
    let mut x = 0x2545_F491_u32;
    (0..frames * channels)
        .map(|i| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let ramp = i64::from((i / channels) as i32 % 2000) - 1000;
            let noise = i64::from((x >> 20) as i32) - 2048;
            let v = (ramp << depth) / 4096 + (noise << depth) / 16384;
            let top = (1i64 << (depth - 1)) - 1;
            (v.clamp(-top - 1, top) as i32) << (width - depth)
        })
        .collect()
}

fn samples(format: SampleFormat, v: &[i32]) -> Samples {
    match format {
        S16 => Samples::S16(v.iter().map(|&x| x as i16).collect()),
        S24 => Samples::S24(v.to_vec()),
        _ => Samples::S32(v.to_vec()),
    }
}

fn frame(spec: &AudioSpec, s: Samples) -> AudioFrame {
    AudioFrame {
        samples: s,
        ..AudioFrame::new(spec)
    }
}

/// Encodes `v` in pieces of `piece` frames; returns the packets and the final cookie.
fn encode(
    spec: &AudioSpec,
    options: &EncodeOptions,
    v: &[i32],
    piece: usize,
) -> (Vec<Packet>, Vec<u8>) {
    let mut e = encoder(spec, options).unwrap();
    let n = usize::from(spec.layout().channels());
    let mut packets = Vec::new();
    let mut p = Packet::default();
    for chunk in v.chunks(piece * n) {
        e.send_frame(&frame(spec, samples(spec.format(), chunk)))
            .unwrap();
        while e.receive_packet(&mut p).unwrap() {
            packets.push(p.clone());
        }
    }
    e.drain();
    while e.receive_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    (packets, e.extradata().to_vec())
}

fn decode(spec: &AudioSpec, cookie: &[u8], packets: &[Packet]) -> Vec<i32> {
    let mut info = StreamInfo::new(
        0,
        CodecId::Alac,
        spec.clone(),
        TimeBase::new(1, i64::from(spec.sample_rate())).unwrap(),
    );
    info.extradata = cookie.to_vec();
    let mut d = decoder(&info).unwrap();
    let mut out = Vec::new();
    let mut f = AudioFrame::new(spec);
    for p in packets {
        d.send_packet(p).unwrap();
        while d.receive_frame(&mut f).unwrap() {
            match &f.samples {
                Samples::S16(s) => out.extend(s.iter().map(|&x| i32::from(x))),
                Samples::S24(s) | Samples::S32(s) => out.extend_from_slice(s),
                _ => unreachable!(),
            }
        }
    }
    d.drain();
    assert!(!d.receive_frame(&mut f).unwrap(), "nothing is held back");
    out
}

fn check(format: SampleFormat, depth: u32, channels: u8, frames: usize, piece: usize) {
    let spec = spec(format, channels);
    let options = EncodeOptions {
        bits_per_sample: Some(depth as u8),
        ..EncodeOptions::default()
    };
    let v = signal(frames, usize::from(channels), depth, format.bits() as u32);
    let (packets, cookie) = encode(&spec, &options, &v, piece);
    assert_eq!(
        decode(&spec, &cookie, &packets),
        v,
        "{format:?} {depth} {channels}ch"
    );
}

#[test]
fn every_depth_and_channel_count_comes_back_exactly() {
    for channels in 1..=8 {
        check(S16, 16, channels, 9000, 1000);
        check(S24, 24, channels, 9000, 4096);
        check(S32, 32, channels, 5000, 777);
    }
    check(S24, 20, 2, 9000, 1500);
    check(S16, 12, 2, 5000, 1500);
}

/// A layout that only names ALAC's surround pair differently, or names nothing, is coded as
/// ALAC's own: the same bytes, every channel at its index.
#[test]
fn a_renamed_or_unlabelled_layout_is_coded_exactly_as_alacs_own() {
    let own = spec(S16, 6);
    let v = signal(5000, 6, 16, 16);
    let (want, want_cookie) = encode(&own, &EncodeOptions::default(), &v, 5000);
    for layout in [
        ChannelLayout::from_name("5.1-back").unwrap(),
        ChannelLayout::unpositioned(6).unwrap(),
    ] {
        let spec = AudioSpec::new(44100, S16, layout.clone()).unwrap();
        let (packets, cookie) = encode(&spec, &EncodeOptions::default(), &v, 5000);
        assert_eq!(packets, want, "{layout:?}");
        assert_eq!(cookie, want_cookie);
    }
}

#[test]
fn packets_hold_4096_frames_and_only_the_last_is_shorter() {
    let spec = spec(S16, 2);
    let v = signal(10_000, 2, 16, 16);
    // The same packets however the frames arrive.
    let (a, ca) = encode(&spec, &EncodeOptions::default(), &v, 10_000);
    let (b, cb) = encode(&spec, &EncodeOptions::default(), &v, 333);
    assert_eq!(a, b);
    assert_eq!(ca, cb);
    let shape: Vec<_> = a.iter().map(|p| (p.pts, p.duration)).collect();
    assert_eq!(shape, [(0, 4096), (4096, 4096), (8192, 1808)]);
    // The last packet finished the cookie.
    let c = crossfade_alac::Config::parse(&ca).unwrap();
    assert!(c.avg_bit_rate > 0);
}

#[test]
fn nothing_in_nothing_out() {
    let (packets, cookie) = encode(&spec(S16, 2), &EncodeOptions::default(), &[], 1);
    assert!(packets.is_empty());
    assert_eq!(cookie.len(), 24);
}

#[test]
fn a_frame_that_does_not_match_the_encoder_is_refused() {
    let mut e = encoder(&spec(S16, 2), &EncodeOptions::default()).unwrap();
    let r = e.send_frame(&frame(&spec(S16, 1), Samples::S16(vec![0; 10])));
    assert!(matches!(r, Err(Error::Unsupported { .. })), "{r:?}");
    let r = e.send_frame(&frame(&spec(S16, 2), Samples::S16(vec![0; 3])));
    assert!(matches!(r, Err(Error::Malformed { .. })), "{r:?}");
}

#[test]
fn a_sample_wider_than_the_coded_depth_is_malformed() {
    let spec = spec(S24, 1);
    let mut e = encoder(&spec, &EncodeOptions::default()).unwrap();
    let r = e.send_frame(&frame(&spec, Samples::S24(vec![1 << 23])));
    assert!(matches!(r, Err(Error::Malformed { .. })), "{r:?}");
}
