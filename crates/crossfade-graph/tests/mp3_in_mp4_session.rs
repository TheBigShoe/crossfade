//! MP3 inside MP4 seeks exactly: seek-then-decode equals decode-then-slice, bit for bit,
//! through `DecodeSession`, even at the lowest bitrates, where a frame's bit reservoir reaches
//! back more frames than any fixed pre-roll. The MP4 files are built here from the frames of the
//! `tests/fixtures/mp3` files (one whole frame per sample).

use crossfade_core::{AudioFrame, Demuxer, Limits, Packet, Samples};
use crossfade_format::open_demuxer;
use crossfade_graph::DecodeSession;
use crossfade_testkit::memory::MemoryIo;
use std::io::Cursor;

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn full(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    bx(kind, &[&[0u8; 4][..], body].concat())
}

fn be(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

/// An MP4 file holding `frames` of `channels` channels at `rate`, `spf` samples each, in one
/// chunk.
fn mp4(frames: &[Vec<u8>], rate: u32, channels: u16, spf: u32) -> Vec<u8> {
    let mut entry = vec![0u8; 6];
    entry.extend(1u16.to_be_bytes());
    entry.extend([0; 8]);
    entry.extend(channels.to_be_bytes());
    entry.extend(16u16.to_be_bytes());
    entry.extend([0; 4]);
    entry.extend(be(rate << 16));
    let stsd = full(b"stsd", &[&be(1)[..], &bx(b".mp3", &entry)].concat());
    let stts = full(b"stts", &[be(1), be(frames.len() as u32), be(spf)].concat());
    let stsc = full(
        b"stsc",
        &[be(1), be(1), be(frames.len() as u32), be(1)].concat(),
    );
    let mut stsz = [be(0), be(frames.len() as u32)].concat();
    for f in frames {
        stsz.extend(be(f.len() as u32));
    }
    let stsz = full(b"stsz", &stsz);
    let moov = |first: u32| {
        let stco = full(b"stco", &[be(1), be(first)].concat());
        let stbl = bx(
            b"stbl",
            &[stsd.clone(), stts.clone(), stsc.clone(), stsz.clone(), stco].concat(),
        );
        let mdhd = full(b"mdhd", &[&[0u8; 8][..], &be(rate), &[0; 8]].concat());
        let hdlr = full(b"hdlr", &[&[0u8; 4][..], b"soun", &[0; 13]].concat());
        let mdia = bx(b"mdia", &[mdhd, hdlr, bx(b"minf", &stbl)].concat());
        let mvhd = full(b"mvhd", &[&[0u8; 8][..], &be(rate), &[0; 84]].concat());
        bx(b"moov", &[mvhd, bx(b"trak", &mdia)].concat())
    };
    let ftyp = bx(b"ftyp", b"M4A \0\0\0\0M4A mp42isom\0\0\0\0");
    let first = (ftyp.len() + moov(0).len() + 8) as u32;
    let mdat = bx(b"mdat", &frames.concat());
    [ftyp, moov(first), mdat].concat()
}

/// The MP4 file for an MP3 fixture: its frames, rate, channels and samples per frame.
fn wrapped(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../crossfade-format/tests/fixtures/mp3/{name}.mp3",
        env!("CARGO_MANIFEST_DIR")
    );
    let (_, mut d) = open_demuxer(
        Box::new(Cursor::new(std::fs::read(path).unwrap())),
        &Limits::default(),
    )
    .unwrap();
    let (rate, channels) = {
        let a = &d.streams()[0].audio;
        (a.sample_rate(), a.layout().channels())
    };
    let mut p = Packet::default();
    let mut frames = Vec::new();
    let mut spf = 0;
    while d.read_packet(&mut p).unwrap() {
        spf = p.duration as u32;
        frames.push(p.data.clone());
    }
    mp4(&frames, rate, channels, spf)
}

fn session(bytes: Vec<u8>) -> DecodeSession {
    let mut io = MemoryIo::new();
    io.insert("in.m4a", bytes);
    DecodeSession::open("in.m4a", &mut io, &Limits::default()).unwrap()
}

fn rest(s: &mut DecodeSession) -> Vec<f32> {
    let mut f = AudioFrame::new(&s.stream().audio);
    let mut out = Vec::new();
    while s.next_frame(&mut f).unwrap() {
        let Samples::F32(v) = &f.samples else {
            unreachable!()
        };
        out.extend_from_slice(v);
    }
    out
}

#[test]
fn seek_then_decode_equals_decode_then_slice_for_mp3_in_mp4() {
    for name in ["mpeg2-8k-burst", "mpeg2-8k", "mpeg25", "mpeg1-32k"] {
        let mut s = session(wrapped(name));
        let ch = usize::from(s.stream().audio.layout().channels());
        let whole = rest(&mut s);
        let frames = (whole.len() / ch) as u64;
        // Targets all along the stream at varying offsets, in no particular order.
        let mut targets: Vec<u64> = (0..frames).step_by(frames as usize / 60 | 1).collect();
        targets.extend([0, 1, frames - 1, frames]);
        targets.reverse();
        for target in targets {
            s.seek(target).unwrap();
            assert_eq!(s.position(), target, "{name}");
            let got = rest(&mut s);
            let want = &whole[target as usize * ch..];
            assert_eq!(got.len(), want.len(), "{name} {target}");
            assert!(
                got.iter()
                    .zip(want)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{name}: seek to {target} differs"
            );
        }
    }
}
