//! The MP4 reader and the ALAC decoder together, on files `afconvert` made from signals this
//! repository regenerates (see `tests/fixtures/mp4/README.md`): every decode must equal the
//! source PCM exactly.

#[path = "support/mp4_signals.rs"]
mod mp4_signals;

use crossfade_codec::decoder_for;
use crossfade_core::{
    AudioFrame, Demuxer, Error, Limits, Packet, SampleFormat, Samples, StreamInfo,
};
use crossfade_format::FormatId;
use crossfade_testkit::io::FaultySource;
use std::io::Cursor;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/mp4/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn open(bytes: Vec<u8>, seekable: bool) -> Result<Box<dyn Demuxer>, Error> {
    let src = FaultySource::new(Cursor::new(bytes));
    let src = if seekable { src } else { src.not_seekable() };
    let (id, d) = crossfade_format::open_demuxer(Box::new(src), &Limits::default())?;
    assert_eq!(id, FormatId::Mp4);
    Ok(d)
}

/// The true depth of a stream: its raw bits, or the container's.
fn depth(s: &StreamInfo) -> u32 {
    u32::from(
        s.bits_per_raw_sample
            .unwrap_or(s.audio.format().bits() as u8),
    )
}

/// Raw sample values with the left-justification undone.
fn unjustify(frame: &AudioFrame, bits: u32, out: &mut Vec<i32>) {
    match &frame.samples {
        Samples::S16(v) => out.extend(v.iter().map(|&s| i32::from(s) >> (16 - bits))),
        Samples::S24(v) => out.extend(v.iter().map(|&s| s >> (24 - bits))),
        Samples::S32(v) => out.extend(v.iter().map(|&s| s >> (32 - bits))),
        other => panic!("unexpected {other:?}"),
    }
}

/// Every packet, checked to follow on from the one before.
fn packets(d: &mut dyn Demuxer) -> Vec<Packet> {
    let mut out: Vec<Packet> = Vec::new();
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        if let Some(last) = out.last() {
            assert_eq!(p.pts, last.pts + last.duration as i64, "gap");
        }
        out.push(p.clone());
    }
    out
}

/// Decodes `packets` and returns the samples on the timeline (from pts 0, up to `frames`).
fn decode(stream: &StreamInfo, packets: &[Packet]) -> Vec<i32> {
    let mut dec = decoder_for(stream).unwrap();
    let mut frame = AudioFrame::new(&stream.audio);
    let mut out = Vec::new();
    let channels = usize::from(stream.audio.layout().channels());
    for p in packets {
        dec.send_packet(p).unwrap();
        while dec.receive_frame(&mut frame).unwrap() {
            assert_eq!(frame.pts, p.pts);
            assert_eq!(frame.frames() as u64, p.duration, "{}", p.pts);
            unjustify(&frame, depth(stream), &mut out);
        }
    }
    dec.drain();
    assert!(!dec.receive_frame(&mut frame).unwrap());
    out.truncate(stream.frames.unwrap() as usize * channels);
    out
}

const ALAC: &[(&str, &str)] = &[
    ("alac-mono16.m4a", "mono16"),
    ("alac-stereo16.m4a", "stereo16"),
    ("alac-stereo20.m4a", "stereo20"),
    ("alac-stereo24.m4a", "stereo24"),
    ("alac-stereo32.m4a", "stereo32"),
    ("alac-ch3.m4a", "ch3"),
    ("alac-ch4.m4a", "ch4"),
    ("alac-ch5.m4a", "ch5"),
    ("alac-ch6.m4a", "ch6"),
    ("alac-ch7.m4a", "ch7"),
    ("alac-ch8.m4a", "ch8"),
    ("alac-noise16.m4a", "noise16"),
    ("alac-silence.m4a", "silence"),
];

#[test]
fn every_alac_fixture_decodes_to_its_source_exactly() {
    for &(file, signal) in ALAC {
        let want = mp4_signals::signal(signal);
        let mut d = open(fixture(file), false).unwrap();
        let s = d.streams()[0].clone();
        assert_eq!(s.codec, crossfade_core::CodecId::Alac, "{file}");
        assert_eq!(s.audio.sample_rate(), want.rate, "{file}");
        assert_eq!(
            s.audio.layout().to_mask(),
            Some(mp4_signals::mask(want.channels)),
            "{file}"
        );
        assert_eq!(depth(&s), u32::from(want.bits), "{file}");
        let format = match want.bits {
            16 => SampleFormat::S16,
            20 | 24 => SampleFormat::S24,
            _ => SampleFormat::S32,
        };
        assert_eq!(s.audio.format(), format, "{file}");
        assert_eq!(s.frames, Some(want.frames() as u64), "{file}");
        assert_eq!(s.extradata.len(), 24, "{file}");
        let p = packets(&mut *d);
        assert_eq!(p[0].pts, 0, "{file}");
        // An odd length: the last packet is a partial frame.
        assert!(p.last().unwrap().duration < 4096, "{file}");
        assert_eq!(decode(&s, &p), want.samples, "{file}");
    }
}

/// Whether an ALAC packet starts with an element in an uncompressed (escape) frame: the flag
/// is the last of the four header bits after the 3-bit tag, 4-bit instance and 12 zero bits.
fn escaped(packet: &[u8]) -> bool {
    packet[2] & 0x02 != 0
}

#[test]
fn loud_noise_makes_escape_frames_and_tones_do_not() {
    let mut noise = open(fixture("alac-noise16.m4a"), false).unwrap();
    assert!(packets(&mut *noise).iter().any(|p| escaped(&p.data)));
    let mut tones = open(fixture("alac-stereo16.m4a"), false).unwrap();
    assert!(!packets(&mut *tones).iter().any(|p| escaped(&p.data)));
}

/// The top-level boxes of a file: (type, whole box).
fn top_boxes(bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let size = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        out.push((bytes[i + 4..i + 8].try_into().unwrap(), &bytes[i..i + size]));
        i += size;
    }
    out
}

/// The same file with `moov` moved after `mdat` and the chunk offsets moved to match.
fn moov_last(bytes: &[u8]) -> Vec<u8> {
    let boxes = top_boxes(bytes);
    let pick = |kind: &[u8; 4]| boxes.iter().find(|(k, _)| k == kind).unwrap().1;
    let (ftyp, moov, mdat) = (pick(b"ftyp"), pick(b"moov"), pick(b"mdat"));
    let old = bytes.windows(4).position(|w| w == b"mdat").unwrap() - 4;
    let new = ftyp.len();
    let mut moov = moov.to_vec();
    let at = moov.windows(4).position(|w| w == b"stco").unwrap() + 4;
    let n = u32::from_be_bytes(moov[at + 4..at + 8].try_into().unwrap()) as usize;
    for k in 0..n {
        let i = at + 8 + 4 * k;
        let v = u32::from_be_bytes(moov[i..i + 4].try_into().unwrap());
        moov[i..i + 4].copy_from_slice(&(v - old as u32 + new as u32).to_be_bytes());
    }
    [ftyp, mdat, &moov].concat()
}

#[test]
fn moov_first_works_on_a_pipe_and_moov_last_needs_a_seekable_input() {
    let first = fixture("alac-stereo16.m4a");
    // afconvert writes moov first.
    let order: Vec<[u8; 4]> = top_boxes(&first).iter().map(|b| b.0).collect();
    assert_eq!(order, [*b"ftyp", *b"moov", *b"free", *b"mdat"]);
    let last = moov_last(&first);
    let order: Vec<[u8; 4]> = top_boxes(&last).iter().map(|b| b.0).collect();
    assert_eq!(order, [*b"ftyp", *b"mdat", *b"moov"]);
    let want = packets(&mut *open(first, false).unwrap());
    let mut d = open(last.clone(), true).unwrap();
    assert_eq!(packets(&mut *d), want);
    let s = d.streams()[0].clone();
    assert_eq!(decode(&s, &want), mp4_signals::signal("stereo16").samples);
    match open(last, false) {
        Err(Error::Unsupported { what }) => assert!(what.contains("after the audio"), "{what}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn aac_in_mp4_is_described_and_its_decoder_refused_unless_built_in() {
    // afinfo: 2 ch, 44100 Hz, aac, 1024 frames/packet, 12 packets,
    // "audio 10007 valid frames + 2112 priming + 169 remainder = 12288".
    let mut d = open(fixture("aac-stereo16.m4a"), false).unwrap();
    let s = d.streams()[0].clone();
    assert_eq!(s.codec, crossfade_core::CodecId::Aac);
    assert_eq!(s.extradata, [0x12, 0x10]);
    assert_eq!(s.audio.sample_rate(), 44100);
    assert_eq!(s.audio.layout().channels(), 2);
    assert_eq!(s.audio.format(), SampleFormat::F32);
    assert_eq!(s.frames, Some(10007));
    let p = packets(&mut *d);
    assert_eq!(p.len(), 12);
    assert_eq!(p[0].pts, -2112);
    assert!(p.iter().all(|p| p.duration == 1024));
    // A build with the `aac` feature decodes it (tests/aac_mp4.rs); one without says why not.
    if let Err(e) = decoder_for(&s) {
        assert_eq!(
            e,
            Error::Unsupported {
                what: "aac decoding is not part of this build (AAC is patent-encumbered); build \
                       with --features aac"
                    .into()
            }
        );
    }
}

/// Seeks the demuxer, resets the decoder, decodes from there and drops what lies before
/// `target` and past the end of the timeline.
fn decode_from(
    d: &mut dyn Demuxer,
    dec: &mut dyn crossfade_core::Decoder,
    target: i64,
) -> Vec<i32> {
    let s = d.streams()[0].clone();
    let channels = usize::from(s.audio.layout().channels());
    let landed = d.seek(target).unwrap();
    assert!(landed <= target.max(0), "{landed} > {target}");
    dec.reset();
    let (mut p, mut frame) = (Packet::default(), AudioFrame::new(&s.audio));
    let mut out = Vec::new();
    let end = s.frames.unwrap() as i64;
    while d.read_packet(&mut p).unwrap() {
        dec.send_packet(&p).unwrap();
        while dec.receive_frame(&mut frame).unwrap() {
            let mut raw = Vec::new();
            unjustify(&frame, depth(&s), &mut raw);
            for (i, f) in raw.chunks(channels).enumerate() {
                let pts = frame.pts + i as i64;
                if pts >= target.max(0) && pts < end {
                    out.extend_from_slice(f);
                }
            }
        }
    }
    out
}

#[test]
fn seek_then_decode_equals_decode_then_slice() {
    for (file, signal) in [("alac-stereo16.m4a", "stereo16"), ("alac-ch5.m4a", "ch5")] {
        let want = mp4_signals::signal(signal);
        let mut d = open(fixture(file), true).unwrap();
        let s = d.streams()[0].clone();
        let mut dec = decoder_for(&s).unwrap();
        let frames = want.frames() as i64;
        let mut seed = 0x5EEDu64;
        let mut targets = vec![0, 1, 4095, 4096, 4097, frames - 1, frames, frames + 9, -3];
        for _ in 0..20 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            targets.push((seed >> 33) as i64 % (frames + 1));
        }
        for target in targets {
            let got = decode_from(&mut *d, &mut *dec, target);
            let from = target.clamp(0, frames) as usize * want.channels;
            assert_eq!(got, want.samples[from..], "{file} {target}");
        }
    }
}
