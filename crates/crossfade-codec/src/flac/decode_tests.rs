//! Adapter tests: what the engine sees (formats, packets, error variants and texts). The
//! decoder itself is tested in `crossfade-flac`.

use super::*;
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Packet, SampleFormat, Samples, TimeBase,
};
use crossfade_flac::bits::BitWriter;
use crossfade_flac::crc::crc16;
use crossfade_flac::frame::{self, ChannelAssignment, FrameHeader};
use crossfade_flac::md5::Md5;

// ---- building blocks: streams, frames and subframes assembled bit by bit ----

fn block_info(rate: u32, channels: u8, bits: u8, total: u64, md5: [u8; 16]) -> StreamInfoBlock {
    StreamInfoBlock {
        min_block_size: 16,
        max_block_size: 4096,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: rate,
        channels,
        bits_per_sample: bits,
        total_samples: total,
        md5,
    }
}

fn stream_for(b: &StreamInfoBlock) -> StreamInfo {
    let format = match b.bits_per_sample {
        0..=8 => SampleFormat::U8,
        9..=16 => SampleFormat::S16,
        17..=24 => SampleFormat::S24,
        _ => SampleFormat::S32,
    };
    let layout = if b.channels == 2 {
        ChannelLayout::stereo()
    } else {
        ChannelLayout::unpositioned(u16::from(b.channels)).unwrap()
    };
    StreamInfo {
        extradata: b.write().to_vec(),
        ..StreamInfo::new(
            0,
            CodecId::Flac,
            AudioSpec::new(b.sample_rate, format, layout).unwrap(),
            TimeBase::new(1, i64::from(b.sample_rate)).unwrap(),
        )
    }
}

fn stream(rate: u32, channels: u8, bits: u8) -> StreamInfo {
    stream_for(&block_info(rate, channels, bits, 0, [0; 16]))
}

/// A frame around `body`: header with its CRC-8 and the CRC-16 trailer.
fn frame_around(
    block: u32,
    channels: ChannelAssignment,
    bits: Option<u8>,
    rate: Option<u32>,
    body: Vec<u8>,
) -> Vec<u8> {
    let h = FrameHeader {
        variable_block_size: false,
        block_size: block,
        sample_rate: rate,
        channels,
        bits_per_sample: bits,
        number: 0,
        header_len: 0,
    };
    let mut f = Vec::new();
    frame::write_header(&h, &mut f);
    f.extend(body);
    let crc = crc16(&f);
    f.extend_from_slice(&crc.to_be_bytes());
    f
}

fn body(build: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut w = BitWriter::new();
    build(&mut w);
    w.byte_align();
    w.into_bytes()
}

fn decode(info: &StreamInfo, frame: &[u8]) -> Result<AudioFrame, Error> {
    let mut d = decoder(info)?;
    d.send_packet(&Packet {
        stream: 0,
        pts: 77,
        duration: 0,
        data: frame.to_vec(),
    })?;
    let mut out = AudioFrame::new(&info.audio);
    assert!(d.receive_frame(&mut out)?);
    assert_eq!(out.pts, 77);
    Ok(out)
}

fn s16(f: &AudioFrame) -> Vec<i16> {
    match &f.samples {
        Samples::S16(v) => v.clone(),
        other => panic!("not S16: {other:?}"),
    }
}

fn unsupported(r: Result<AudioFrame, Error>) -> String {
    match r {
        Err(Error::Unsupported { what }) => what,
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

fn malformed(r: Result<AudioFrame, Error>) -> String {
    match r {
        Err(Error::Malformed { what }) => what,
        Err(Error::UnexpectedEof { .. }) => "eof".into(),
        other => panic!("expected Malformed, got {other:?}"),
    }
}

/// Subframe header: pad bit, 6-bit type, wasted-bits flag (and its unary count).
fn sub_header(w: &mut BitWriter, kind: u64, wasted: u32) {
    w.write_bits(0, 1);
    w.write_bits(kind, 6);
    if wasted == 0 {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        w.write_unary(wasted - 1);
    }
}

fn sub_verbatim(w: &mut BitWriter, x: &[i64], bps: u32) {
    sub_header(w, 1, 0);
    for &v in x {
        w.write_signed(v, bps);
    }
}

/// A wandering signal of `n` samples within `bits`, deterministic.
fn wave(n: usize, bits: u32, seed: i64) -> Vec<i64> {
    let max = (1i64 << (bits - 1)) - 1;
    let mut s = seed;
    (0..n)
        .map(|i| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407 + i as i64)
                .rem_euclid(1 << 40);
            let v = (s >> 8) % (max / 4 + 1);
            (v * if i % 3 == 0 { -1 } else { 1 } + (i as i64 * (max / 200 + 1)) % (max / 2 + 1))
                .clamp(-max - 1, max)
        })
        .collect()
}

fn mono_frame(x: &[i64], bps: u8, build: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    frame_around(
        x.len() as u32,
        ChannelAssignment::Independent(1),
        Some(bps),
        None,
        body(build),
    )
}

// ---- stereo ----

fn stereo_frame(
    assign: ChannelAssignment,
    bps: u8,
    a: &[i64],
    b: &[i64],
    bps_a: u32,
    bps_b: u32,
) -> Vec<u8> {
    frame_around(
        a.len() as u32,
        assign,
        Some(bps),
        None,
        body(|w| {
            sub_verbatim(w, a, bps_a);
            sub_verbatim(w, b, bps_b);
        }),
    )
}

#[test]
fn samples_are_left_justified_in_the_container() {
    for bits in [4u8, 8, 9, 12, 16, 17, 20, 24, 25, 32] {
        let max = (1i64 << (bits - 1)) - 1;
        let x = [0, 1, -1, max, -max - 1, max / 2];
        // Depths a frame header cannot name are inherited from STREAMINFO.
        let code = matches!(bits, 8 | 12 | 16 | 20 | 24 | 32).then_some(bits);
        let f = frame_around(
            6,
            ChannelAssignment::Independent(1),
            code,
            None,
            body(|w| sub_verbatim(w, &x, u32::from(bits))),
        );
        let out = decode(&stream(48000, 1, bits), &f).unwrap();
        let shift = |container: u32| u32::from(bits).abs_diff(container);
        match (&out.samples, bits) {
            (Samples::U8(v), 4..=8) => {
                let want: Vec<u8> = x
                    .iter()
                    .map(|&s| ((s << (8 - u32::from(bits))) + 128) as u8)
                    .collect();
                assert_eq!(*v, want, "{bits}");
            }
            (Samples::S16(v), 9..=16) => {
                let want: Vec<i16> = x.iter().map(|&s| (s << shift(16)) as i16).collect();
                assert_eq!(*v, want, "{bits}");
            }
            (Samples::S24(v), 17..=24) => {
                let want: Vec<i32> = x.iter().map(|&s| (s << shift(24)) as i32).collect();
                assert_eq!(*v, want, "{bits}");
            }
            (Samples::S32(v), 25..=32) => {
                let want: Vec<i32> = x.iter().map(|&s| (s << shift(32)) as i32).collect();
                assert_eq!(*v, want, "{bits}");
            }
            other => panic!("{bits} bits gave {other:?}"),
        }
    }
}

#[test]
fn a_frame_that_changes_the_stream_parameters_is_unsupported() {
    let x = wave(16, 16, 1);
    let mono = mono_frame(&x, 16, |w| sub_verbatim(w, &x, 16));
    let info = stream(44100, 2, 16);
    let m = unsupported(decode(&info, &mono));
    assert!(
        m.contains("channel") && m.contains('1') && m.contains('2'),
        "{m}"
    );

    let m = unsupported(decode(&stream(44100, 1, 24), &mono));
    assert!(
        m.contains("bit depth") && m.contains("16") && m.contains("24"),
        "{m}"
    );

    let f = frame_around(
        16,
        ChannelAssignment::Independent(1),
        Some(16),
        Some(48000),
        body(|w| sub_verbatim(w, &x, 16)),
    );
    let m = unsupported(decode(&stream(44100, 1, 16), &f));
    assert!(
        m.contains("sample rate") && m.contains("48000") && m.contains("44100"),
        "{m}"
    );
}

#[test]
fn a_bad_crc_or_header_is_malformed() {
    let x = wave(16, 16, 1);
    let good = mono_frame(&x, 16, |w| sub_verbatim(w, &x, 16));
    let info = stream(44100, 1, 16);
    for at in [good.len() - 1, good.len() - 2, good.len() / 2] {
        let mut bad = good.clone();
        bad[at] ^= 0x10;
        assert!(malformed(decode(&info, &bad)).contains("CRC"), "byte {at}");
    }
    let mut bad = good.clone();
    bad[0] = 0;
    assert!(matches!(decode(&info, &bad), Err(Error::Malformed { .. })));
    assert!(matches!(
        decode(&info, &good[..4]),
        Err(Error::Malformed { .. } | Error::UnexpectedEof { .. })
    ));
    assert!(matches!(
        decode(&info, &[]),
        Err(Error::Malformed { .. } | Error::UnexpectedEof { .. })
    ));
}

#[test]
fn zero_bytes_after_the_crc_are_ignored() {
    // The container reader cannot tell zero padding after the last frame from the frame itself.
    let x = wave(16, 16, 1);
    let mut f = mono_frame(&x, 16, |w| sub_verbatim(w, &x, 16));
    f.extend([0u8; 7]);
    let out = decode(&stream(44100, 1, 16), &f).unwrap();
    assert_eq!(s16(&out), x.iter().map(|&v| v as i16).collect::<Vec<_>>());
    // Anything but zeros there is not padding.
    *f.last_mut().unwrap() = 1;
    assert!(matches!(
        decode(&stream(44100, 1, 16), &f),
        Err(Error::Malformed { .. })
    ));
}

#[test]
fn a_bad_stream_description_is_rejected() {
    let mut info = stream(44100, 2, 16);
    info.extradata.pop();
    assert!(matches!(decoder(&info), Err(Error::Malformed { .. })));
    let mut info = stream(44100, 2, 16);
    info.extradata.clear();
    assert!(matches!(decoder(&info), Err(Error::Malformed { .. })));
    // The description and STREAMINFO disagree.
    let mut info = stream(44100, 2, 16);
    info.extradata = block_info(44100, 1, 16, 0, [0; 16]).write().to_vec();
    assert!(matches!(decoder(&info), Err(Error::Malformed { .. })));
}

// ---- MD5 and totals ----

fn md5_of(samples: &[i64], bytes: usize) -> [u8; 16] {
    let mut m = Md5::new();
    for s in samples {
        m.update(&s.to_le_bytes()[..bytes]);
    }
    m.finalize()
}

fn run(info: &StreamInfo, frames: &[Vec<u8>]) -> (Vec<Result<bool, Error>>, usize) {
    let mut d = decoder(info).unwrap();
    let mut out = AudioFrame::new(&info.audio);
    let mut got = 0;
    for f in frames {
        d.send_packet(&Packet {
            data: f.clone(),
            ..Packet::default()
        })
        .unwrap();
        while d.receive_frame(&mut out).unwrap() {
            got += 1;
        }
    }
    d.drain();
    let mut tail = Vec::new();
    for _ in 0..3 {
        tail.push(d.receive_frame(&mut out));
    }
    (tail, got)
}

fn two_frames() -> (Vec<Vec<u8>>, Vec<i64>) {
    let l = wave(48, 12, 30);
    let r = wave(48, 12, 31);
    let mut frames = Vec::new();
    let mut inter = Vec::new();
    for range in [0..30, 30..48] {
        let (a, b) = (&l[range.clone()], &r[range]);
        frames.push(stereo_frame(
            ChannelAssignment::Independent(2),
            12,
            a,
            b,
            12,
            12,
        ));
        for (x, y) in a.iter().zip(b) {
            inter.extend([*x, *y]);
        }
    }
    (frames, inter)
}

#[test]
fn a_matching_md5_and_total_end_cleanly() {
    let (frames, inter) = two_frames();
    // The hash is over the raw 12-bit values in two bytes each, not the shifted output.
    let info = stream_for(&block_info(44100, 2, 12, 48, md5_of(&inter, 2)));
    let (tail, got) = run(&info, &frames);
    assert_eq!(got, 2);
    assert_eq!(tail, vec![Ok(false), Ok(false), Ok(false)]);
}

#[test]
fn a_wrong_md5_is_reported_once() {
    let (frames, inter) = two_frames();
    let mut md5 = md5_of(&inter, 2);
    md5[3] ^= 1;
    let info = stream_for(&block_info(44100, 2, 12, 48, md5));
    let (tail, _) = run(&info, &frames);
    assert_eq!(
        tail,
        vec![
            Err(Error::Malformed {
                what: "flac: decoded audio does not match the MD5 in the stream header".into()
            }),
            Ok(false),
            Ok(false)
        ]
    );
}

#[test]
fn a_zero_md5_and_a_zero_total_mean_no_check() {
    let (frames, _) = two_frames();
    let info = stream_for(&block_info(44100, 2, 12, 0, [0; 16]));
    let (tail, got) = run(&info, &frames);
    assert_eq!((got, tail), (2, vec![Ok(false); 3]));
}

#[test]
fn a_total_that_does_not_match_is_reported() {
    let (frames, _) = two_frames();
    let info = stream_for(&block_info(44100, 2, 12, 100, [0; 16]));
    let (tail, _) = run(&info, &frames);
    assert_eq!(
        tail[0],
        Err(Error::Malformed {
            what: "flac: stream ended after 48 of 100 samples".into()
        })
    );
    assert_eq!(tail[1], Ok(false));
    let info = stream_for(&block_info(44100, 2, 12, 10, [0; 16]));
    let (tail, _) = run(&info, &frames);
    assert_eq!(
        tail[0],
        Err(Error::Malformed {
            what: "flac: stream ended after 48 of 10 samples".into()
        })
    );
}

#[test]
fn a_failed_frame_suppresses_the_end_of_stream_checks() {
    let (frames, _) = two_frames();
    let info = stream_for(&block_info(44100, 2, 12, 48, [9; 16]));
    let mut d = decoder(&info).unwrap();
    let mut bad = frames[0].clone();
    *bad.last_mut().unwrap() ^= 1;
    let p = |data: &[u8]| Packet {
        data: data.to_vec(),
        ..Packet::default()
    };
    assert!(d.send_packet(&p(&bad)).is_err());
    d.drain();
    let mut out = AudioFrame::new(&info.audio);
    assert_eq!(d.receive_frame(&mut out), Ok(false));
}

#[test]
fn frames_are_handed_over_without_reallocating() {
    let x = wave(256, 16, 50);
    let f = mono_frame(&x, 16, |w| sub_verbatim(w, &x, 16));
    let info = stream(44100, 1, 16);
    let mut d = decoder(&info).unwrap();
    let mut out = AudioFrame::new(&info.audio);
    let mut seen = std::collections::HashSet::new();
    for i in 0..10 {
        d.send_packet(&Packet {
            data: f.clone(),
            pts: i,
            ..Packet::default()
        })
        .unwrap();
        assert!(d.receive_frame(&mut out).unwrap());
        assert_eq!(out.pts, i);
        let Samples::S16(v) = &out.samples else {
            panic!()
        };
        seen.insert(v.as_ptr() as usize);
        assert_eq!(v.len(), 256);
    }
    // Two buffers trade places; nothing new appears after the start.
    assert!(seen.len() <= 2, "{} distinct buffers", seen.len());
    assert_eq!(d.receive_frame(&mut out), Ok(false));
}

/// A worker panic is a bug, not the input; the engine has no variant of its own for it, so it
/// arrives as `Unsupported` with the text that says what happened.
#[test]
fn an_internal_error_keeps_its_text() {
    assert_eq!(
        engine_error(crossfade_flac::Error::Internal("flac decoder: boom".into())),
        Error::Unsupported {
            what: "internal error: flac decoder: boom".into()
        }
    );
}
