//! MP3 files made by `lame` (tests/fixtures/mp3/README.md) through the reader and the decoder:
//! exact gapless length and alignment against the regenerated source, frozen digests of the
//! decoded output, tags, and agreement with short `mpg123` references.

#[path = "support/mp3_signals.rs"]
mod mp3_signals;

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Demuxer, Limits, MediaSource, Packet, Samples, StreamInfo, Tags};
use crossfade_format::{FormatId, open_demuxer};
use crossfade_testkit::io::FaultySource;
use mp3_signals::{fixture, source};
use std::io::Cursor;

fn bytes(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/mp3/{name}.mp3",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).unwrap()
}

/// Every decoded frame (pts and samples), as the decoder emits them, untrimmed.
struct Decoded {
    stream: StreamInfo,
    tags: Tags,
    frames: Vec<(i64, Vec<f32>)>,
}

fn open(data: &[u8], seekable: bool) -> Box<dyn Demuxer> {
    let src: Box<dyn MediaSource> = if seekable {
        Box::new(Cursor::new(data.to_vec()))
    } else {
        Box::new(FaultySource::new(Cursor::new(data.to_vec())).not_seekable())
    };
    let (format, d) = open_demuxer(src, &Limits::default()).unwrap();
    assert_eq!(format, FormatId::Mp3);
    d
}

fn decode(data: &[u8], seekable: bool) -> Decoded {
    let mut d = open(data, seekable);
    let stream = d.streams()[0].clone();
    let mut dec = decoder_for(&stream).unwrap();
    let mut p = Packet::default();
    let mut f = AudioFrame::new(&stream.audio);
    let mut frames = Vec::new();
    while d.read_packet(&mut p).unwrap() {
        dec.send_packet(&p).unwrap();
        while dec.receive_frame(&mut f).unwrap() {
            let Samples::F32(s) = &f.samples else {
                panic!()
            };
            frames.push((f.pts, s.clone()));
        }
    }
    Decoded {
        stream,
        tags: d.tags().clone(),
        frames,
    }
}

/// What the engine's `Timeline` keeps: from pts 0, up to `frames`.
fn timeline(d: &Decoded) -> Vec<f32> {
    let ch = usize::from(d.stream.audio.layout().channels());
    let end = d.stream.frames.map_or(i64::MAX, |n| n as i64);
    let mut out = Vec::new();
    for (pts, s) in &d.frames {
        for (i, frame) in s.chunks(ch).enumerate() {
            let t = pts + i as i64;
            if (0..end).contains(&t) {
                out.extend_from_slice(frame);
            }
        }
    }
    out
}

fn raw(d: &Decoded) -> Vec<f32> {
    d.frames
        .iter()
        .flat_map(|(_, s)| s.iter().copied())
        .collect()
}

/// The lag (decoded minus source, in sample frames) at which channel `c` best matches.
fn best_lag(
    decoded: &[f32],
    src: &[i16],
    ch: usize,
    c: usize,
    range: std::ops::RangeInclusive<i64>,
) -> i64 {
    let src: Vec<f64> = src
        .iter()
        .skip(c)
        .step_by(ch)
        .map(|&v| f64::from(v) / 32768.0)
        .collect();
    let dec: Vec<f64> = decoded
        .iter()
        .skip(c)
        .step_by(ch)
        .map(|&v| f64::from(v))
        .collect();
    range
        .max_by(|&a, &b| {
            let corr = |lag: i64| -> f64 {
                (0..src.len())
                    .filter_map(|i| {
                        let j = usize::try_from(i as i64 + lag).ok()?;
                        Some(src[i] * dec.get(j)?)
                    })
                    .sum()
            };
            corr(a).total_cmp(&corr(b))
        })
        .unwrap()
}

fn snr_db(decoded: &[f32], src: &[i16]) -> f64 {
    let (mut sig, mut err) = (0f64, 0f64);
    for (&d, &s) in decoded.iter().zip(src) {
        let s = f64::from(s) / 32768.0;
        sig += s * s;
        err += (f64::from(d) - s) * (f64::from(d) - s);
    }
    10.0 * (sig / err).log10()
}

#[test]
fn lame_tagged_files_are_exactly_the_source_length_and_aligned() {
    for name in ["cbr128", "cbr320", "vbr", "mpeg2", "free", "tagged"] {
        let fx = fixture(name);
        let src = source(fx);
        let ch = usize::from(fx.channels);
        for seekable in [true, false] {
            let d = decode(&bytes(name), seekable);
            assert_eq!(d.stream.frames, Some(fx.frames as u64), "{name}");
            let kept = timeline(&d);
            assert_eq!(kept.len(), src.len(), "{name}: the gapless length");
            for c in 0..ch {
                assert_eq!(
                    best_lag(&kept, &src, ch, c, -40..=40),
                    0,
                    "{name} channel {c}"
                );
            }
            let snr = snr_db(&kept, &src);
            assert!(snr > 20.0, "{name}: {snr:.1} dB");
        }
    }
}

#[test]
fn without_lame_figures_only_the_decoders_delay_is_known() {
    // Small frames (and `lame -t`) carry no info frame: the stream starts 529 frames in and
    // keeps LAME's own 576 frames of delay, so the source sits 576 frames late.
    for name in ["notag", "mpeg1-32k", "mpeg2-8k", "mpeg25"] {
        let fx = fixture(name);
        let src = source(fx);
        let d = decode(&bytes(name), true);
        let spf = d.frames[0].1.len() / usize::from(fx.channels);
        let total = d.frames.len() * spf;
        assert_eq!(d.stream.frames, Some(total as u64 - 529), "{name}");
        assert_eq!(d.frames[0].0, -529, "{name}");
        let kept = timeline(&d);
        assert_eq!(
            best_lag(&kept, &src, usize::from(fx.channels), 0, 560..=600),
            576,
            "{name}"
        );
        // On a pipe the length is unknown.
        assert_eq!(decode(&bytes(name), false).stream.frames, None, "{name}");
    }
}

/// FNV-1a over the bits of every decoded sample, priming and padding included.
fn digest(samples: &[f32]) -> u64 {
    samples.iter().fold(0xcbf2_9ce4_8422_2325, |h, s| {
        s.to_bits()
            .to_le_bytes()
            .iter()
            .fold(h, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
    })
}

#[test]
fn decoded_output_is_frozen() {
    let got: Vec<(&str, u64)> = mp3_signals::FIXTURES
        .iter()
        .map(|f| (f.name, digest(&raw(&decode(&bytes(f.name), true)))))
        .collect();
    // Rewritten only on purpose: a change here is a change in what the decoder gives.
    let want: &[(&str, u64)] = &[
        ("cbr128", 2271097031343297424),
        ("cbr320", 16993911738096980363),
        ("vbr", 6095075397068891942),
        // The same audio frames as cbr128: only the info frame and the tags differ.
        ("notag", 2271097031343297424),
        ("mpeg1-32k", 11905247866050172579),
        ("mpeg2", 8437896555722022947),
        ("mpeg2-8k", 8054511417465821658),
        ("mpeg25", 237907240453474107),
        ("free", 13413933754520653377),
        ("tagged", 2271097031343297424),
    ];
    assert_eq!(got, want);
}

#[test]
fn agrees_with_mpg123_on_a_frame_of_each_mpeg_version() {
    // tests/fixtures/mp3/*.mpg123.f32: 576 sample frames of `mpg123 -e f32 --no-gapless`
    // output, from the start of the third frame.
    for (name, spf) in [("cbr128", 1152), ("mpeg2", 576), ("mpeg25", 576)] {
        let path = format!(
            "{}/tests/fixtures/mp3/{name}.mpg123.f32",
            env!("CARGO_MANIFEST_DIR")
        );
        let file = std::fs::read(path).unwrap();
        let reference: Vec<f32> = file
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect();
        let d = decode(&bytes(name), true);
        let ch = usize::from(d.stream.audio.layout().channels());
        let ours = &raw(&d)[2 * spf * ch..][..reference.len()];
        let worst = ours
            .iter()
            .zip(&reference)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(worst < 1e-5, "{name}: {worst:e}");
    }
}

#[test]
fn id3v2_3_utf16_tags_and_id3v1() {
    let d = decode(&bytes("tagged"), true);
    let tags: Vec<(&str, &str)> = d
        .tags
        .iter()
        .map(|t| (t.key.as_str(), t.value.as_str()))
        .collect();
    assert_eq!(
        tags,
        [
            ("TITLE", "Title"),
            ("ARTIST", "Artist"),
            ("ALBUM", "Album"),
            ("DATE", "2024"),
            ("TRACKNUMBER", "3/9"),
            ("GENRE", "Rock"),
            ("COMMENT", "comment"),
        ]
    );
    // The audio is untouched by the tags around it.
    assert_eq!(timeline(&d).len(), 2 * 22050);
}

#[test]
fn an_id3v2_4_tag_with_utf16_in_front_of_a_file() {
    let title: Vec<u8> = [
        &[1u8, 0xFF, 0xFE][..],
        &"Ünïcødé"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    ]
    .concat();
    let mut frame = b"TIT2".to_vec();
    frame.extend([0, 0, 0, title.len() as u8, 0, 0]);
    frame.extend(&title);
    let mut tag = b"ID3\x04\x00\x00\x00\x00\x00".to_vec();
    tag.push(frame.len() as u8);
    tag.extend(&frame);
    let plain = decode(&bytes("cbr128"), true);
    let tagged = decode(&[tag, bytes("cbr128")].concat(), true);
    assert_eq!(tagged.tags.get("TITLE"), Some("Ünïcødé"));
    assert_eq!(raw(&tagged), raw(&plain));
}
