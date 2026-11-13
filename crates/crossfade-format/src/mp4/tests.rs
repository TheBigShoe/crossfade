//! The reader on hand-built files; `tests/mp4_fixtures.rs` has files made by `afconvert`.

use super::*;
use crossfade_core::{
    AudioSpec, Channel, ChannelLayout, CodecId, Packet, SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_testkit::io::FaultySource;
use proptest::prelude::*;
use std::io::Cursor;

pub(super) fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn full(kind: &[u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version, 0, 0, 0];
    b.extend_from_slice(body);
    bx(kind, &b)
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// An audio sample entry, version 0, around `children`.
fn sound_entry(kind: &[u8; 4], channels: u16, rate: u32, children: &[u8]) -> Vec<u8> {
    let mut b = vec![0; 6];
    b.extend_from_slice(&1u16.to_be_bytes());
    b.extend_from_slice(&[0; 8]);
    b.extend_from_slice(&channels.to_be_bytes());
    b.extend_from_slice(&16u16.to_be_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&(rate.min(0xFFFF) << 16).to_be_bytes());
    b.extend_from_slice(children);
    bx(kind, &b)
}

fn alac_config(depth: u8, channels: u8, rate: u32) -> [u8; 24] {
    crossfade_alac::Config {
        frame_length: 4096,
        compatible_version: 0,
        bit_depth: depth,
        pb: 40,
        mb: 10,
        kb: 14,
        channels,
        max_run: 255,
        max_frame_bytes: 0,
        avg_bit_rate: 0,
        sample_rate: rate,
    }
    .write()
}

fn alac_entry(depth: u8, channels: u8, rate: u32) -> Vec<u8> {
    let config = full(b"alac", 0, &alac_config(depth, channels, rate));
    sound_entry(b"alac", u16::from(channels), rate, &config)
}

/// An `esds` with the given object type and decoder specific info.
fn esds(object_type: u8, info: &[u8]) -> Vec<u8> {
    let mut dsi = vec![0x05, info.len() as u8];
    dsi.extend_from_slice(info);
    let mut dcd = vec![object_type, 0x15, 0, 0x18, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    dcd.extend_from_slice(&dsi);
    let mut dcd_desc = vec![0x04, 0x80, 0x80, 0x80, dcd.len() as u8];
    dcd_desc.extend_from_slice(&dcd);
    let mut es = vec![0, 0, 0];
    es.extend_from_slice(&dcd_desc);
    es.extend_from_slice(&[0x06, 1, 2]);
    let mut es_desc = vec![0x03, es.len() as u8];
    es_desc.extend_from_slice(&es);
    full(b"esds", 0, &es_desc)
}

fn mp4a_entry(object_type: u8, info: &[u8], channels: u16, rate: u32) -> Vec<u8> {
    sound_entry(b"mp4a", channels, rate, &esds(object_type, info))
}

/// A one-track file built from parts.
#[derive(Clone)]
pub(super) struct File {
    pub(super) entry: Vec<u8>,
    pub(super) handler: [u8; 4],
    pub(super) timescale: u32,
    pub(super) movie_timescale: u32,
    /// Sample bytes and durations (media timescale).
    pub(super) samples: Vec<(Vec<u8>, u32)>,
    pub(super) per_chunk: usize,
    /// (segment_duration, media_time).
    pub(super) edit: Option<(u32, i32)>,
    pub(super) ilst: Option<Vec<u8>>,
    pub(super) moov_last: bool,
    pub(super) co64: bool,
    pub(super) stz2: bool,
    pub(super) extra_moov: Vec<u8>,
}

impl File {
    pub(super) fn alac(samples: usize) -> Self {
        Self {
            entry: alac_entry(16, 2, 44100),
            handler: *b"soun",
            timescale: 44100,
            movie_timescale: 44100,
            samples: (0..samples)
                .map(|i| (vec![i as u8; 3 + i % 5], 4096))
                .collect(),
            per_chunk: 3,
            edit: None,
            ilst: None,
            moov_last: false,
            co64: false,
            stz2: false,
            extra_moov: Vec::new(),
        }
    }

    fn stbl(&self, first_offset: u64) -> Vec<u8> {
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend_from_slice(&self.entry);
        let mut stts = Vec::new();
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &(_, d) in &self.samples {
            match runs.last_mut() {
                Some((n, delta)) if *delta == d => *n += 1,
                _ => runs.push((1, d)),
            }
        }
        stts.extend_from_slice(&(runs.len() as u32).to_be_bytes());
        for (n, d) in runs {
            stts.extend_from_slice(&n.to_be_bytes());
            stts.extend_from_slice(&d.to_be_bytes());
        }
        let chunks: Vec<&[(Vec<u8>, u32)]> = self.samples.chunks(self.per_chunk).collect();
        let mut stsc = 1u32.to_be_bytes().to_vec();
        stsc.extend_from_slice(&1u32.to_be_bytes());
        stsc.extend_from_slice(&(self.per_chunk as u32).to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        let sizes = if self.stz2 {
            let mut b = vec![0, 0, 0, 16];
            b.extend_from_slice(&(self.samples.len() as u32).to_be_bytes());
            for (s, _) in &self.samples {
                b.extend_from_slice(&(s.len() as u16).to_be_bytes());
            }
            full(b"stz2", 0, &b)
        } else {
            let mut b = 0u32.to_be_bytes().to_vec();
            b.extend_from_slice(&(self.samples.len() as u32).to_be_bytes());
            for (s, _) in &self.samples {
                b.extend_from_slice(&(s.len() as u32).to_be_bytes());
            }
            full(b"stsz", 0, &b)
        };
        let mut offsets = (chunks.len() as u32).to_be_bytes().to_vec();
        let mut at = first_offset;
        for c in &chunks {
            if self.co64 {
                offsets.extend_from_slice(&at.to_be_bytes());
            } else {
                offsets.extend_from_slice(&(at as u32).to_be_bytes());
            }
            at += c.iter().map(|(s, _)| s.len() as u64).sum::<u64>();
        }
        let stco = full(if self.co64 { b"co64" } else { b"stco" }, 0, &offsets);
        bx(
            b"stbl",
            &cat(&[
                &full(b"stsd", 0, &stsd),
                &full(b"stts", 0, &stts),
                &full(b"stsc", 0, &stsc),
                &sizes,
                &stco,
            ]),
        )
    }

    fn moov(&self, first_offset: u64) -> Vec<u8> {
        let mut mvhd = vec![0; 8];
        mvhd.extend_from_slice(&self.movie_timescale.to_be_bytes());
        mvhd.extend_from_slice(&[0; 84]);
        let mut mdhd = vec![0; 8];
        mdhd.extend_from_slice(&self.timescale.to_be_bytes());
        mdhd.extend_from_slice(&[0; 8]);
        let mut hdlr = vec![0; 4];
        hdlr.extend_from_slice(&self.handler);
        hdlr.extend_from_slice(&[0; 13]);
        let minf = bx(b"minf", &self.stbl(first_offset));
        let mdia = bx(
            b"mdia",
            &cat(&[&full(b"mdhd", 0, &mdhd), &full(b"hdlr", 0, &hdlr), &minf]),
        );
        let edts = match self.edit {
            Some((duration, media_time)) => {
                let mut e = 1u32.to_be_bytes().to_vec();
                e.extend_from_slice(&duration.to_be_bytes());
                e.extend_from_slice(&media_time.to_be_bytes());
                e.extend_from_slice(&0x0001_0000u32.to_be_bytes());
                bx(b"edts", &full(b"elst", 0, &e))
            }
            None => Vec::new(),
        };
        let trak = bx(b"trak", &cat(&[&edts, &mdia]));
        let udta = match &self.ilst {
            Some(items) => {
                let hdlr = full(b"hdlr", 0, &cat(&[&[0; 4], b"mdirappl", &[0; 9]]));
                bx(
                    b"udta",
                    &full(b"meta", 0, &cat(&[&hdlr, &bx(b"ilst", items)])),
                )
            }
            None => Vec::new(),
        };
        bx(
            b"moov",
            &cat(&[&full(b"mvhd", 0, &mvhd), &self.extra_moov, &trak, &udta]),
        )
    }

    pub(super) fn bytes(&self) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"M4A \0\0\0\0M4A mp42isom\0\0\0\0");
        let data: Vec<u8> = self.samples.iter().flat_map(|(s, _)| s.clone()).collect();
        let mdat = bx(b"mdat", &data);
        if self.moov_last {
            let first = (ftyp.len() + 8) as u64;
            return cat(&[&ftyp, &mdat, &self.moov(first)]);
        }
        let len = self.moov(0).len();
        let first = (ftyp.len() + len + 8) as u64;
        cat(&[&ftyp, &self.moov(first), &mdat])
    }
}

fn open_bytes(bytes: Vec<u8>, seekable: bool) -> Result<Box<dyn Demuxer>, Error> {
    let src = FaultySource::new(Cursor::new(bytes));
    let src = if seekable { src } else { src.not_seekable() };
    open(Box::new(src), &Limits::default())
}

fn pipe(f: &File) -> Box<dyn Demuxer> {
    open_bytes(f.bytes(), false).unwrap()
}

fn packets(d: &mut dyn Demuxer) -> Vec<Packet> {
    let mut out = Vec::new();
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        out.push(p.clone());
    }
    out
}

fn message(r: Result<Box<dyn Demuxer>, Error>) -> String {
    match r {
        Err(Error::Unsupported { what } | Error::Malformed { what }) => what,
        Err(e) => panic!("unexpected {e:?}"),
        Ok(_) => panic!("opened"),
    }
}

#[test]
fn an_alac_track_on_a_pipe() {
    let mut f = File::alac(7);
    f.samples[6].1 = 1000;
    let mut d = pipe(&f);
    let s = &d.streams()[0];
    let audio = AudioSpec::new(44100, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    let want = StreamInfo {
        frames: Some(6 * 4096 + 1000),
        extradata: alac_config(16, 2, 44100).to_vec(),
        ..StreamInfo::new(0, CodecId::Alac, audio, TimeBase::new(1, 44100).unwrap())
    };
    assert_eq!(s, &want);
    let got = packets(&mut *d);
    assert_eq!(got.len(), 7);
    for (i, p) in got.iter().enumerate() {
        assert_eq!(p.data, f.samples[i].0, "{i}");
        assert_eq!(p.pts, i as i64 * 4096);
        assert_eq!(p.duration, if i == 6 { 1000 } else { 4096 });
    }
    assert!(d.tags().is_empty());
}

#[test]
fn every_table_form_gives_the_same_packets() {
    let base = File::alac(11);
    let want = packets(&mut *pipe(&base));
    for (co64, stz2, per_chunk) in [(true, false, 3), (false, true, 1), (true, true, 11)] {
        let f = File {
            co64,
            stz2,
            per_chunk,
            ..base.clone()
        };
        assert_eq!(packets(&mut *pipe(&f)), want, "{co64} {stz2} {per_chunk}");
    }
}

#[test]
fn moov_after_mdat_needs_an_input_that_can_seek() {
    let f = File {
        moov_last: true,
        ..File::alac(5)
    };
    let want = packets(&mut *pipe(&File::alac(5)));
    let mut d = open_bytes(f.bytes(), true).unwrap();
    assert_eq!(packets(&mut *d), want);
    let why = message(open_bytes(f.bytes(), false));
    assert!(why.contains("after the audio (mdat)"), "{why}");
    assert!(why.contains("cannot"), "{why}");
}

#[test]
fn fragmented_and_encrypted_files_are_refused_by_name() {
    let f = File {
        extra_moov: bx(b"mvex", &[]),
        ..File::alac(1)
    };
    assert_eq!(
        message(open_bytes(f.bytes(), false)),
        "fragmented mp4 is not supported yet"
    );
    let mut moof = bx(b"ftyp", b"iso6");
    moof.extend(bx(b"moof", &[]));
    assert_eq!(
        message(open_bytes(moof, false)),
        "fragmented mp4 is not supported yet"
    );
    let enca = File {
        entry: sound_entry(b"enca", 2, 44100, &[]),
        ..File::alac(1)
    };
    let sinf = File {
        entry: sound_entry(b"mp4a", 2, 44100, &bx(b"sinf", &[])),
        ..File::alac(1)
    };
    for f in [enca, sinf] {
        assert_eq!(
            message(open_bytes(f.bytes(), false)),
            "mp4: encrypted audio is not supported"
        );
    }
}

#[test]
fn other_codecs_are_refused_by_their_four_character_name() {
    for (kind, name) in [(b"Opus", "Opus"), (b"fLaC", "fLaC"), (b"ac-3", "ac-3")] {
        let f = File {
            entry: sound_entry(kind, 2, 48000, &[]),
            ..File::alac(1)
        };
        assert_eq!(
            message(open_bytes(f.bytes(), false)),
            format!("mp4: the '{name}' codec is not supported")
        );
    }
    let f = File {
        entry: mp4a_entry(0xA9, &[], 2, 48000),
        ..File::alac(1)
    };
    assert_eq!(
        message(open_bytes(f.bytes(), false)),
        "mp4: mp4a with object type 0xA9 is not supported"
    );
}

#[test]
fn aac_takes_rate_and_channels_from_its_audio_specific_config() {
    // The entry claims mono at 22050; the config says 48 kHz 5.1.
    let asc = [0x11, 0xB0];
    for object_type in [0x40, 0x66, 0x67, 0x68] {
        let f = File {
            entry: mp4a_entry(object_type, &asc, 1, 22050),
            timescale: 48000,
            ..File::alac(2)
        };
        let d = pipe(&f);
        let s = &d.streams()[0];
        assert_eq!(s.codec, CodecId::Aac);
        assert_eq!(s.extradata, asc);
        assert_eq!(s.audio.sample_rate(), 48000);
        assert_eq!(s.audio.format(), SampleFormat::F32);
        assert_eq!(s.audio.layout(), &ChannelLayout::from_name("5.1").unwrap());
        assert_eq!(s.bits_per_raw_sample, None);
    }
    let pce = File {
        entry: mp4a_entry(0x40, &[0x11, 0x80], 2, 48000),
        ..File::alac(1)
    };
    // Channel configuration 0 and no program config element: the config is cut off.
    assert!(message(open_bytes(pce.bytes(), false)).contains("cut off"));
    let none = File {
        entry: sound_entry(b"mp4a", 2, 48000, &esds(0x40, &[])[..14]),
        ..File::alac(1)
    };
    assert!(open_bytes(none.bytes(), false).is_err());
}

#[test]
fn mp3_in_mp4() {
    for (kind, object_type) in [(b"mp4a", 0x6B), (b"mp4a", 0x69), (b".mp3", 0)] {
        let entry = if kind == b"mp4a" {
            mp4a_entry(object_type, &[], 2, 44100)
        } else {
            sound_entry(b".mp3", 2, 44100, &[])
        };
        let f = File {
            entry,
            ..File::alac(1)
        };
        let s = pipe(&f).streams()[0].clone();
        assert_eq!(s.codec, CodecId::Mp3);
        assert!(s.extradata.is_empty());
        assert_eq!(
            (s.audio.sample_rate(), s.audio.format()),
            (44100, SampleFormat::F32)
        );
        assert_eq!(s.audio.layout(), &ChannelLayout::stereo());
    }
}

#[test]
fn alac_depths_and_layouts() {
    for (depth, format, raw) in [
        (16, SampleFormat::S16, None),
        (20, SampleFormat::S24, Some(20)),
        (24, SampleFormat::S24, None),
        (32, SampleFormat::S32, None),
    ] {
        let f = File {
            entry: alac_entry(depth, 1, 96000),
            ..File::alac(1)
        };
        let s = pipe(&f).streams()[0].clone();
        assert_eq!((s.audio.format(), s.bits_per_raw_sample), (format, raw));
        // The config's rate: the entry's 16.16 field cannot hold 96000.
        assert_eq!(s.audio.sample_rate(), 96000);
        assert_eq!(s.audio.layout(), &ChannelLayout::mono());
    }
    let f = File {
        entry: alac_entry(16, 8, 48000),
        ..File::alac(1)
    };
    use Channel::*;
    assert_eq!(
        pipe(&f).streams()[0].audio.layout().positions(),
        Some(
            &[
                FrontLeft,
                FrontRight,
                FrontCenter,
                Lfe,
                FrontLeftOfCenter,
                FrontRightOfCenter,
                SideLeft,
                SideRight
            ][..]
        )
    );
    let bad = File {
        entry: alac_entry(8, 2, 48000),
        ..File::alac(1)
    };
    assert!(message(open_bytes(bad.bytes(), false)).contains("bit depth"));
}

#[test]
fn the_quicktime_wave_wrapping_is_accepted() {
    let config = full(b"alac", 0, &alac_config(24, 2, 48000));
    let wave = bx(
        b"wave",
        &cat(&[&bx(b"frma", b"alac"), &config, &[0, 0, 0, 8, 0, 0, 0, 0]]),
    );
    let f = File {
        entry: sound_entry(b"alac", 2, 48000, &wave),
        ..File::alac(1)
    };
    assert_eq!(pipe(&f).streams()[0].audio.format(), SampleFormat::S24);
    let wave = bx(
        b"wave",
        &cat(&[
            &bx(b"frma", b"mp4a"),
            &bx(b"mp4a", &[0; 4]),
            &esds(0x40, &[0x12, 0x10]),
        ]),
    );
    let f = File {
        entry: sound_entry(b"mp4a", 2, 44100, &wave),
        ..File::alac(1)
    };
    assert_eq!(pipe(&f).streams()[0].codec, CodecId::Aac);
}

fn smpb(priming: u32, padding: u32, length: u64) -> Vec<u8> {
    let text = format!(" 00000000 {priming:08X} {padding:08X} {length:016X} 00000000 00000000");
    free_form("iTunSMPB", text.as_bytes())
}

fn free_form(name: &str, value: &[u8]) -> Vec<u8> {
    let mean = full(b"mean", 0, b"com.apple.iTunes");
    let nm = full(b"name", 0, name.as_bytes());
    let data = bx(b"data", &cat(&[&[0, 0, 0, 1, 0, 0, 0, 0], value]));
    bx(b"----", &cat(&[&mean, &nm, &data]))
}

fn aac_file(edit: Option<(u32, i32)>, tag: Option<Vec<u8>>) -> File {
    File {
        entry: mp4a_entry(0x40, &[0x12, 0x10], 2, 44100),
        samples: (0..12).map(|i| (vec![i; 6], 1024)).collect(),
        edit,
        ilst: tag,
        ..File::alac(0)
    }
}

fn timeline(f: &File) -> (i64, Option<u64>) {
    let mut d = pipe(f);
    let frames = d.streams()[0].frames;
    let first = packets(&mut *d)[0].pts;
    (first, frames)
}

#[test]
fn gapless_from_the_edit_list_or_itunsmpb_with_the_edit_list_first() {
    // 12 packets of 1024: 12288 frames.
    assert_eq!(
        timeline(&aac_file(Some((10007, 2112)), None)),
        (-2112, Some(10007))
    );
    assert_eq!(
        timeline(&aac_file(None, Some(smpb(2112, 169, 10007)))),
        (-2112, Some(10007))
    );
    // They disagree: the edit list wins.
    assert_eq!(
        timeline(&aac_file(Some((9000, 1024)), Some(smpb(2112, 169, 10007)))),
        (-1024, Some(9000))
    );
    assert_eq!(timeline(&aac_file(None, None)), (0, Some(12288)));
    // An open segment plays to the end; a length past the end stops there.
    assert_eq!(
        timeline(&aac_file(Some((0, 2112)), None)),
        (-2112, Some(10176))
    );
    assert_eq!(
        timeline(&aac_file(None, Some(smpb(2112, 0, 99999)))),
        (-2112, Some(10176))
    );
    // Priming past the end is not believed: the next source, or none.
    assert_eq!(
        timeline(&aac_file(Some((10, 20000)), Some(smpb(2112, 169, 10007)))),
        (-2112, Some(10007))
    );
    assert_eq!(
        timeline(&aac_file(None, Some(smpb(20000, 0, 1)))),
        (0, Some(12288))
    );
    // A malformed tag, or an edit list that is not a single plain edit, is ignored.
    assert_eq!(
        timeline(&aac_file(None, Some(free_form("iTunSMPB", b"00 zz")))),
        (0, Some(12288))
    );
    assert_eq!(timeline(&aac_file(Some((100, -1)), None)), (0, Some(12288)));
}

#[test]
fn the_edit_list_length_is_in_the_movie_timescale() {
    // Movie timescale 1000: 227 ms is 10010.7 frames at 44.1 kHz, rounded to 10011.
    let f = File {
        movie_timescale: 1000,
        ..aac_file(Some((227, 2112)), None)
    };
    assert_eq!(timeline(&f), (-2112, Some(10011)));
}

#[test]
fn a_media_timescale_other_than_the_rate_scales_rounding_down() {
    // ALAC at 44.1 kHz in a 90 kHz media timescale: 4096 frames are 8359.18... ticks; the
    // file says 8359, then 8360, then 100. Positions are ticks * 0.49 rounded down: 0, 4095
    // (4095.91), 8192 (8192.31) and the end 8241 (8241.31).
    let mut f = File::alac(3);
    f.timescale = 90000;
    f.samples[0].1 = 8359;
    f.samples[1].1 = 8360;
    f.samples[2].1 = 100;
    let got: Vec<(i64, u64)> = packets(&mut *pipe(&f))
        .iter()
        .map(|p| (p.pts, p.duration))
        .collect();
    assert_eq!(got, [(0, 4095), (4095, 4097), (8192, 49)]);
    assert_eq!(pipe(&f).streams()[0].frames, Some(8241));
}

fn text_item(kind: &[u8; 4], value: &str) -> Vec<u8> {
    bx(
        kind,
        &bx(
            b"data",
            &cat(&[&[0, 0, 0, 1, 0, 0, 0, 0], value.as_bytes()]),
        ),
    )
}

fn binary_item(kind: &[u8; 4], value: &[u8]) -> Vec<u8> {
    bx(
        kind,
        &bx(b"data", &cat(&[&[0, 0, 0, 0, 0, 0, 0, 0], value])),
    )
}

#[test]
fn ilst_items_map_onto_the_shared_tag_names() {
    let mut utf16 = vec![0, 0, 0, 2, 0, 0, 0, 0];
    for u in "Ünï".encode_utf16() {
        utf16.extend_from_slice(&u.to_be_bytes());
    }
    let items = cat(&[
        &text_item(b"\xA9nam", "Title"),
        &text_item(b"\xA9ART", "Artist"),
        &text_item(b"aART", "Album Artist"),
        &text_item(b"\xA9alb", "Album"),
        &text_item(b"\xA9day", "2026"),
        &text_item(b"\xA9gen", "Ambient"),
        &binary_item(b"gnre", &[0, 18]),
        &binary_item(b"trkn", &[0, 0, 0, 3, 0, 12, 0, 0]),
        &binary_item(b"disk", &[0, 0, 0, 1, 0, 0]),
        &text_item(b"\xA9cmt", "Comment"),
        &text_item(b"\xA9wrt", "Composer"),
        &text_item(b"\xA9too", "afconvert"),
        &bx(
            b"covr",
            &bx(b"data", &[0, 0, 0, 13, 0, 0, 0, 0, 0xFF, 0xD8]),
        ),
        &bx(b"\xA9cmt", &bx(b"data", &utf16)),
        &free_form("REPLAYGAIN_TRACK_GAIN", b"-6.5 dB"),
        &smpb(0, 0, 10),
        &bx(b"\xA9nam", &[0, 0, 0, 3]),
    ]);
    let f = File {
        ilst: Some(items),
        ..File::alac(1)
    };
    let tags = pipe(&f).tags().clone();
    let got: Vec<(&str, &str)> = tags
        .iter()
        .map(|t| (t.key.as_str(), t.value.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("TITLE", "Title"),
            ("ARTIST", "Artist"),
            ("ALBUMARTIST", "Album Artist"),
            ("ALBUM", "Album"),
            ("DATE", "2026"),
            ("GENRE", "Ambient"),
            ("GENRE", "Rock"),
            ("TRACKNUMBER", "3"),
            ("TRACKTOTAL", "12"),
            ("DISCNUMBER", "1"),
            ("COMMENT", "Comment"),
            ("COMPOSER", "Composer"),
            ("ENCODER", "afconvert"),
            ("COMMENT", "Ünï"),
            ("REPLAYGAIN_TRACK_GAIN", "-6.5 dB"),
        ]
    );
}

#[test]
fn a_quicktime_meta_without_version_is_read_too() {
    let hdlr = full(b"hdlr", 0, &cat(&[&[0; 4], b"mdirappl", &[0; 9]]));
    let meta = bx(
        b"meta",
        &cat(&[&hdlr, &bx(b"ilst", &text_item(b"\xA9nam", "T"))]),
    );
    let f = File {
        extra_moov: meta,
        ..File::alac(1)
    };
    let mut want = Tags::new();
    want.push("TITLE", "T");
    assert_eq!(pipe(&f).tags(), &want);
}

fn seekable(f: &File) -> Box<dyn Demuxer> {
    open_bytes(f.bytes(), true).unwrap()
}

#[test]
fn seeking_lands_on_the_packet_with_its_codecs_preroll() {
    // ALAC: no pre-roll; the packet containing the target.
    let mut d = seekable(&File::alac(7));
    for (target, want) in [
        (0, 0),
        (4095, 0),
        (4096, 4096),
        (10000, 8192),
        (-5, 0),
        (i64::MIN, 0),
    ] {
        assert_eq!(d.seek(target), Ok(want), "{target}");
        let mut p = Packet::default();
        assert!(d.read_packet(&mut p).unwrap());
        assert_eq!(p.pts, want);
    }
    // At or past the end: the end, and nothing more to read.
    for target in [7 * 4096, i64::MAX] {
        assert_eq!(d.seek(target), Ok(7 * 4096));
        assert!(!d.read_packet(&mut Packet::default()).unwrap());
    }
    // AAC: one packet earlier, on the primed timeline.
    let mut d = seekable(&aac_file(Some((10007, 2112)), None));
    assert_eq!(d.seek(0), Ok(1024 - 2112));
    assert_eq!(d.seek(5000), Ok(5 * 1024 - 2112));
    assert_eq!(d.seek(-2112), Ok(-2112));
    // MP3: ten packets earlier, never before the first.
    let mp3 = File {
        entry: mp4a_entry(0x6B, &[], 2, 44100),
        samples: (0..30).map(|i| (vec![i; 4], 1152)).collect(),
        ..File::alac(0)
    };
    let mut d = seekable(&mp3);
    assert_eq!(d.seek(20 * 1152 + 5), Ok(10 * 1152));
    assert_eq!(d.seek(3 * 1152), Ok(0));
}

#[test]
fn a_pipe_cannot_seek() {
    let mut d = pipe(&File::alac(3));
    assert!(matches!(d.seek(0), Err(Error::Unsupported { .. })));
}

#[test]
fn a_track_with_no_samples_reads_nothing_and_seeks_to_its_end() {
    let mut d = seekable(&File::alac(0));
    assert_eq!(d.streams()[0].frames, Some(0));
    assert!(!d.read_packet(&mut Packet::default()).unwrap());
    assert_eq!(d.seek(0), Ok(0));
}

#[test]
fn the_first_sound_track_is_read_and_others_ignored() {
    let video = File {
        handler: *b"vide",
        ..File::alac(1)
    };
    assert_eq!(
        message(open_bytes(video.bytes(), false)),
        "mp4: the file has no audio track"
    );
}

fn with_limits(bytes: Vec<u8>, limits: Limits) -> Result<Box<dyn Demuxer>, Error> {
    open(Box::new(Cursor::new(bytes)), &limits)
}

#[test]
fn hostile_sizes_and_counts_are_errors() {
    let good = File::alac(4).bytes();
    // moov larger than the allocation limit.
    let small = Limits {
        max_alloc_bytes: 100,
        ..Limits::default()
    };
    assert!(matches!(
        with_limits(good.clone(), small),
        Err(Error::AllocLimitExceeded { .. })
    ));
    // A sample larger than the packet limit.
    let tight = Limits {
        max_packet_bytes: 4,
        ..Limits::default()
    };
    let mut d = with_limits(good.clone(), tight).unwrap();
    let mut p = Packet::default();
    // Samples of 3 and 4 bytes fit; 5 does not.
    assert!(d.read_packet(&mut p).unwrap());
    assert!(d.read_packet(&mut p).unwrap());
    assert!(matches!(
        d.read_packet(&mut p),
        Err(Error::AllocLimitExceeded { .. })
    ));
    // Nesting deeper than the limit.
    let shallow = Limits {
        max_nesting_depth: 3,
        ..Limits::default()
    };
    assert!(with_limits(good.clone(), shallow).is_err());
    // No moov at all; a truncated file; a box shorter than its header.
    let ftyp = bx(b"ftyp", b"M4A ");
    assert!(message(open_bytes(ftyp.clone(), false)).contains("no moov"));
    assert!(matches!(
        open_bytes(good[..good.len() / 3].to_vec(), false),
        Err(Error::UnexpectedEof { .. } | Error::Malformed { .. })
    ));
    let mut short = ftyp.clone();
    short.extend_from_slice(&[0, 0, 0, 4, b'f', b'r', b'e', b'e']);
    assert!(message(open_bytes(short, false)).contains("shorter than its header"));
    // The samples run past the end of the file.
    let mut cut = good.clone();
    cut.truncate(cut.len() - 2);
    let mut d = open_bytes(cut, false).unwrap();
    let mut p = Packet::default();
    let mut n = 0;
    let end = loop {
        match d.read_packet(&mut p) {
            Ok(true) => n += 1,
            other => break other,
        }
    };
    assert_eq!(n, 3);
    assert!(matches!(end, Err(Error::UnexpectedEof { .. })));
}

/// The offset of `kind` in `bytes` (the first one).
fn at(bytes: &[u8], kind: &[u8; 4]) -> usize {
    bytes.windows(4).position(|w| w == kind).unwrap()
}

#[test]
fn inconsistent_tables_are_errors() {
    let good = File::alac(4).bytes();
    let patch = |kind: &[u8; 4], offset: usize, value: u32| {
        let mut b = good.clone();
        let i = at(&b, kind) + 4 + offset;
        b[i..i + 4].copy_from_slice(&value.to_be_bytes());
        message(open_bytes(b, false))
    };
    // stts: entry count past the box; one sample short.
    assert!(patch(b"stts", 4, 1000).contains("more entries than bytes"));
    assert!(patch(b"stts", 8, 3).contains("fewer samples"));
    // stsc: not starting at chunk 1; another sample description; too few samples per chunk.
    assert!(patch(b"stsc", 8, 2).contains("start at 1"));
    assert!(patch(b"stsc", 16, 2).contains("more than one sample description"));
    assert!(patch(b"stsc", 12, 1).contains("fewer samples than stsz"));
    // stsz: more samples than entries.
    assert!(patch(b"stsz", 8, 400).contains("more entries than bytes"));
    // A timescale of 0.
    assert!(patch(b"mdhd", 12, 0).contains("timescale is 0"));
}

#[test]
fn open_demuxer_routes_ftyp_here() {
    let bytes = File::alac(2).bytes();
    let src = FaultySource::new(Cursor::new(bytes)).not_seekable();
    let (id, d) = crate::open_demuxer(Box::new(src), &Limits::default()).unwrap();
    assert_eq!(id, crate::FormatId::Mp4);
    assert_eq!(d.streams()[0].codec, CodecId::Alac);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]
    #[test]
    fn damaged_files_never_panic(
        flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
        cut in any::<prop::sample::Index>(),
        moov_last in any::<bool>(),
    ) {
        let mut f = aac_file(Some((10007, 2112)), Some(smpb(2112, 169, 10007)));
        f.moov_last = moov_last;
        let mut bytes = f.bytes();
        for (i, v) in flips {
            let i = i.index(bytes.len());
            bytes[i] = v;
        }
        bytes.truncate(cut.index(bytes.len()) + 1);
        let limits = Limits { max_alloc_bytes: 1 << 16, max_packet_bytes: 1 << 12, ..Limits::default() };
        if let Ok(mut d) = with_limits(bytes, limits) {
            let mut p = Packet::default();
            for _ in 0..64 {
                if !matches!(d.read_packet(&mut p), Ok(true)) { break; }
            }
            let _ = d.seek(3000);
            let _ = d.read_packet(&mut p);
        }
    }
}
