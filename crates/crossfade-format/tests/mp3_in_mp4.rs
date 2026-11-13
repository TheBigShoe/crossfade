//! MP3 inside MP4 (`.mp3` sample entry, one whole frame per sample) decodes through the same
//! engine decoder as an `.mp3` file, to the same samples. The MP4 file is built here from the
//! frames of `tests/fixtures/mp3/cbr128.mp3`.

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, CodecId, Demuxer, Limits, Packet, Samples};
use crossfade_format::{FormatId, open_demuxer};
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

/// An MP4 file holding `frames` (44.1 kHz stereo, 1152 samples each) in one chunk.
fn mp4(frames: &[Vec<u8>]) -> Vec<u8> {
    let mut entry = vec![0u8; 6];
    entry.extend(1u16.to_be_bytes());
    entry.extend([0; 8]);
    entry.extend(2u16.to_be_bytes());
    entry.extend(16u16.to_be_bytes());
    entry.extend([0; 4]);
    entry.extend(be(44100 << 16));
    let stsd = full(b"stsd", &[&be(1)[..], &bx(b".mp3", &entry)].concat());
    let stts = full(
        b"stts",
        &[be(1), be(frames.len() as u32), be(1152)].concat(),
    );
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
        let mdhd = full(b"mdhd", &[&[0u8; 8][..], &be(44100), &[0; 8]].concat());
        let hdlr = full(b"hdlr", &[&[0u8; 4][..], b"soun", &[0; 13]].concat());
        let mdia = bx(b"mdia", &[mdhd, hdlr, bx(b"minf", &stbl)].concat());
        let mvhd = full(b"mvhd", &[&[0u8; 8][..], &be(44100), &[0; 84]].concat());
        bx(b"moov", &[mvhd, bx(b"trak", &mdia)].concat())
    };
    let ftyp = bx(b"ftyp", b"M4A \0\0\0\0M4A mp42isom\0\0\0\0");
    let first = (ftyp.len() + moov(0).len() + 8) as u32;
    let mdat = bx(b"mdat", &frames.concat());
    [ftyp, moov(first), mdat].concat()
}

/// Every packet and the decoded samples, untrimmed.
fn read(d: &mut dyn Demuxer) -> (Vec<Vec<u8>>, Vec<f32>) {
    let stream = d.streams()[0].clone();
    let mut dec = decoder_for(&stream).unwrap();
    let (mut p, mut f) = (Packet::default(), AudioFrame::new(&stream.audio));
    let (mut packets, mut samples) = (Vec::new(), Vec::new());
    while d.read_packet(&mut p).unwrap() {
        packets.push(p.data.clone());
        dec.send_packet(&p).unwrap();
        while dec.receive_frame(&mut f).unwrap() {
            let Samples::F32(s) = &f.samples else {
                unreachable!()
            };
            samples.extend_from_slice(s);
        }
    }
    (packets, samples)
}

#[test]
fn mp3_in_mp4_decodes_like_the_mp3_file() {
    let path = format!(
        "{}/tests/fixtures/mp3/cbr128.mp3",
        env!("CARGO_MANIFEST_DIR")
    );
    let (_, mut file) = open_demuxer(
        Box::new(Cursor::new(std::fs::read(path).unwrap())),
        &Limits::default(),
    )
    .unwrap();
    let (frames, want) = read(&mut *file);
    let (format, mut d) =
        open_demuxer(Box::new(Cursor::new(mp4(&frames))), &Limits::default()).unwrap();
    assert_eq!(format, FormatId::Mp4);
    assert_eq!(d.streams()[0].codec, CodecId::Mp3);
    let (packets, got) = read(&mut *d);
    assert_eq!(packets, frames);
    assert_eq!(got.len(), want.len());
    assert!(
        got.iter()
            .zip(&want)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
}
