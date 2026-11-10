//! `LIST`/`INFO` tags. The builders know nothing about the reader or the writer.

use super::{WavDemuxer, WavMuxer};
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, MediaSink, Muxer as _, Packet,
    SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_testkit::io::FaultySource;
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, Mutex};

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut c = id.to_vec();
    c.extend_from_slice(&(body.len() as u32).to_le_bytes());
    c.extend_from_slice(body);
    if body.len() % 2 == 1 {
        c.push(0);
    }
    c
}

fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = chunks.concat();
    let mut f = b"RIFF".to_vec();
    f.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
    f.extend_from_slice(b"WAVE");
    f.extend_from_slice(&body);
    f
}

/// Mono 16-bit 8000 Hz.
fn fmt() -> Vec<u8> {
    chunk(
        b"fmt ",
        &[1, 0, 1, 0, 0x40, 0x1F, 0, 0, 0x80, 0x3E, 0, 0, 2, 0, 16, 0],
    )
}

fn info(subs: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
    let mut body = b"INFO".to_vec();
    for (id, v) in subs {
        body.extend_from_slice(&chunk(id, v));
    }
    chunk(b"LIST", &body)
}

const PCM: [u8; 8] = [1, 0, 2, 0, 3, 0, 4, 0];

fn kv(d: &dyn Demuxer) -> Vec<(String, String)> {
    d.tags()
        .iter()
        .map(|t| (t.key.clone(), t.value.clone()))
        .collect()
}

fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn open_with(bytes: Vec<u8>, seekable: bool, limits: &Limits) -> Result<WavDemuxer, Error> {
    let src = FaultySource::new(Cursor::new(bytes));
    let src = if seekable { src } else { src.not_seekable() };
    WavDemuxer::new(Box::new(src), limits)
}

fn open(bytes: Vec<u8>, seekable: bool) -> WavDemuxer {
    open_with(bytes, seekable, &Limits::default()).unwrap()
}

fn all_data(d: &mut WavDemuxer) -> Vec<u8> {
    let mut p = Packet::default();
    let mut out = Vec::new();
    while d.read_packet(&mut p).unwrap() {
        out.extend_from_slice(&p.data);
    }
    out
}

#[test]
fn info_before_data_becomes_tags_on_any_source() {
    let f = riff(&[
        fmt(),
        info(&[
            (b"INAM", b"Title\0"),
            (b"IART", b"Artist\0"),
            (b"IPRD", b"Album\0"),
            (b"ICRD", b"2026-11-10\0"),
            (b"ICMT", b"a comment"),
            (b"IGNR", b"Ambient\0"),
            (b"ITRK", b"7\0"),
            (b"ICOP", b"(c) me\0"),
            (b"ISFT", b"ignored\0"),
        ]),
        chunk(b"data", &PCM),
    ]);
    let want = pairs(&[
        ("TITLE", "Title"),
        ("ARTIST", "Artist"),
        ("ALBUM", "Album"),
        ("DATE", "2026-11-10"),
        ("COMMENT", "a comment"),
        ("GENRE", "Ambient"),
        ("TRACKNUMBER", "7"),
        ("COPYRIGHT", "(c) me"),
    ]);
    for seekable in [true, false] {
        let mut d = open(f.clone(), seekable);
        assert_eq!(kv(&d), want, "seekable={seekable}");
        assert_eq!(all_data(&mut d), PCM);
    }
}

#[test]
fn info_after_data_is_read_only_from_a_seekable_source() {
    let f = riff(&[fmt(), chunk(b"data", &PCM), info(&[(b"INAM", b"Late\0")])]);
    let mut seekable = open(f.clone(), true);
    assert_eq!(kv(&seekable), pairs(&[("TITLE", "Late")]));
    // The reader went back to the audio after looking past it.
    assert_eq!(all_data(&mut seekable), PCM);
    let mut pipe = open(f, false);
    assert!(kv(&pipe).is_empty());
    assert_eq!(all_data(&mut pipe), PCM);
}

#[test]
fn info_on_both_sides_of_the_data_accumulates() {
    let f = riff(&[
        fmt(),
        info(&[(b"INAM", b"One\0")]),
        chunk(b"data", &PCM),
        chunk(b"junk", b"odd"),
        info(&[(b"IART", b"Two\0")]),
    ]);
    let d = open(f, true);
    assert_eq!(kv(&d), pairs(&[("TITLE", "One"), ("ARTIST", "Two")]));
}

#[test]
fn text_is_utf8_then_latin1_and_ends_at_the_first_nul() {
    let f = riff(&[
        fmt(),
        info(&[
            (b"INAM", "Café\0".as_bytes()),
            (b"IART", &[b'C', b'a', b'f', 0xE9, 0]),
            (b"IPRD", b"cut\0off"),
            (b"ICMT", b""),
        ]),
        chunk(b"data", &PCM),
    ]);
    let d = open(f, false);
    assert_eq!(
        kv(&d),
        pairs(&[
            ("TITLE", "Café"),
            ("ARTIST", "Café"),
            ("ALBUM", "cut"),
            ("COMMENT", ""),
        ])
    );
}

#[test]
fn other_lists_and_stray_bytes_are_not_tags() {
    let f = riff(&[
        fmt(),
        chunk(b"LIST", b"adtlltxt\0\0\0\0"),
        chunk(b"LIST", b"IN"),
        // A sub-chunk claiming more than the list holds ends the list.
        chunk(
            b"LIST",
            &[&b"INFOINAM\x05\0\0\0ok\0\0"[..], b"IART\xFF\xFF\xFF\xFFxx"].concat(),
        ),
        chunk(b"data", &PCM),
    ]);
    let d = open(f, false);
    assert_eq!(kv(&d), pairs(&[("TITLE", "ok")]));
}

#[test]
fn oversized_info_is_skipped_without_trouble() {
    let big = vec![b'x'; 1 << 20];
    let f = riff(&[
        fmt(),
        info(&[(b"INAM", &big)]),
        info(&[(b"IART", b"small\0")]),
        chunk(b"data", &PCM),
    ]);
    // Over the 1 MiB cap: the first list is skipped, the second read.
    assert_eq!(kv(&open(f, false)), pairs(&[("ARTIST", "small")]));
    // Over the allocation limit: likewise.
    let limits = Limits {
        max_alloc_bytes: 8192,
        ..Limits::default()
    };
    let f = riff(&[
        fmt(),
        info(&[(b"INAM", &vec![b'y'; 9000])]),
        info(&[(b"IART", b"ok\0")]),
        chunk(b"data", &PCM),
    ]);
    let d = open_with(f, false, &limits).unwrap();
    assert_eq!(kv(&d), pairs(&[("ARTIST", "ok")]));
}

#[test]
fn a_hostile_list_size_costs_nothing_and_a_short_file_is_an_error() {
    let mut f = riff(&[fmt()]);
    f.extend_from_slice(b"LIST\xFF\xFF\xFF\xFFINFO");
    for seekable in [true, false] {
        let r = open_with(f.clone(), seekable, &Limits::default());
        assert!(
            matches!(r, Err(Error::UnexpectedEof { .. })),
            "{:?}",
            r.err()
        );
    }
}

#[test]
fn truncation_at_every_byte_never_panics_and_keeps_whole_frames() {
    let f = riff(&[
        fmt(),
        info(&[(b"INAM", b"t\0")]),
        chunk(b"data", &PCM),
        info(&[(b"IART", b"tail\0")]),
    ]);
    for cut in 0..=f.len() as u64 {
        for seekable in [true, false] {
            let src = FaultySource::new(Cursor::new(f.clone())).truncate_at(cut);
            let src = if seekable { src } else { src.not_seekable() };
            if let Ok(mut d) = WavDemuxer::new(Box::new(src), &Limits::default()) {
                let mut p = Packet::default();
                while let Ok(true) = d.read_packet(&mut p) {}
            }
        }
    }
    // Cut inside the audio: the whole frames come out, then the error.
    let data_at = f.windows(4).position(|w| w == b"data").unwrap();
    let mut d = open_with(f[..data_at + 8 + 5].to_vec(), true, &Limits::default()).unwrap();
    let mut p = Packet::default();
    assert!(d.read_packet(&mut p).unwrap());
    assert_eq!(p.data, &PCM[..4]);
    assert!(matches!(
        d.read_packet(&mut p),
        Err(Error::UnexpectedEof { .. })
    ));
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

impl MediaSink for Shared {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(&mut *self.0.lock().unwrap(), buf)
    }
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        std::io::Seek::seek(&mut *self.0.lock().unwrap(), pos)
    }
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn stream(format: SampleFormat, channels: u16, rate: u32) -> StreamInfo {
    let layout = match channels {
        1 => ChannelLayout::mono(),
        2 => ChannelLayout::stereo(),
        n => ChannelLayout::unpositioned(n).unwrap(),
    };
    StreamInfo::new(
        0,
        CodecId::pcm_for(format),
        AudioSpec::new(rate, format, layout).unwrap(),
        TimeBase::new(1, i64::from(rate)).unwrap(),
    )
}

fn tags(p: &[(&str, &str)]) -> Tags {
    let mut t = Tags::new();
    for (k, v) in p {
        t.push(*k, *v);
    }
    t
}

fn write(s: &StreamInfo, t: &Tags, data: &[u8]) -> Vec<u8> {
    let sink = Shared::default();
    let mut m = WavMuxer::with_tags(Box::new(sink.clone()), s, t).unwrap();
    m.write_packet(&Packet {
        data: data.to_vec(),
        ..Packet::default()
    })
    .unwrap();
    m.finish().unwrap();
    let bytes = sink.0.lock().unwrap().get_ref().clone();
    bytes
}

#[test]
fn the_info_chunk_sits_between_the_format_and_the_data() {
    let s = stream(SampleFormat::S16, 1, 8000);
    let got = write(&s, &tags(&[("TITLE", "Hi"), ("ARTIST", "abc")]), &PCM);
    // INAM "Hi\0" + pad, IART "abc\0": LIST size = 4 + 12 + 12.
    let want = riff(&[
        fmt(),
        chunk(
            b"LIST",
            &[
                &b"INFO"[..],
                b"INAM\x03\0\0\0Hi\0\0",
                b"IART\x04\0\0\0abc\0",
            ]
            .concat(),
        ),
        chunk(b"data", &PCM),
    ]);
    assert_eq!(got, want);
    assert_eq!(
        u32::from_le_bytes(got[4..8].try_into().unwrap()) as usize,
        got.len() - 8
    );
}

#[test]
fn keys_come_out_in_a_fixed_order_first_value_wins_unmapped_dropped() {
    let s = stream(SampleFormat::S16, 1, 8000);
    let t = tags(&[
        ("COPYRIGHT", "c"),
        ("REPLAYGAIN_TRACK_GAIN", "-3 dB"),
        ("TRACKNUMBER", "2"),
        ("GENRE", "g"),
        ("COMMENT", "first"),
        ("COMMENT", "second"),
        ("DATE", "2026"),
        ("ALBUM", "al"),
        ("ARTIST", "ar"),
        ("TITLE", "ti"),
    ]);
    let d = open(write(&s, &t, &PCM), false);
    assert_eq!(
        kv(&d),
        pairs(&[
            ("TITLE", "ti"),
            ("ARTIST", "ar"),
            ("ALBUM", "al"),
            ("DATE", "2026"),
            ("COMMENT", "first"),
            ("GENRE", "g"),
            ("TRACKNUMBER", "2"),
            ("COPYRIGHT", "c"),
        ])
    );
}

#[test]
fn without_mapped_tags_the_output_is_exactly_the_untagged_output() {
    for format in [SampleFormat::S16, SampleFormat::F32, SampleFormat::S24] {
        let s = stream(format, 2, 44100);
        let data = vec![7u8; 5 * s.audio.bytes_per_frame()];
        let plain = {
            let sink = Shared::default();
            let mut m = WavMuxer::new(Box::new(sink.clone()), &s).unwrap();
            m.write_packet(&Packet {
                data: data.clone(),
                ..Packet::default()
            })
            .unwrap();
            m.finish().unwrap();
            let bytes = sink.0.lock().unwrap().get_ref().clone();
            bytes
        };
        assert_eq!(write(&s, &Tags::new(), &data), plain, "{format:?}");
        assert_eq!(
            write(&s, &tags(&[("REPLAYGAIN_ALBUM_GAIN", "1 dB")]), &data),
            plain
        );
    }
}

#[test]
fn tags_survive_every_header_shape_and_odd_data_lengths() {
    let t = tags(&[("TITLE", "t"), ("COPYRIGHT", "odd")]);
    for (format, channels, rate, frames) in [
        (SampleFormat::U8, 1u16, 8000u32, 3usize),
        (SampleFormat::S16, 2, 44100, 4),
        (SampleFormat::F32, 2, 48000, 3),
        (SampleFormat::S24, 6, 96000, 2),
    ] {
        let s = stream(format, channels, rate);
        let data: Vec<u8> = (0..frames * s.audio.bytes_per_frame())
            .map(|i| i as u8)
            .collect();
        let bytes = write(&s, &t, &data);
        assert_eq!(bytes.len() % 2, 0);
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            bytes.len() - 8
        );
        let mut d = open(bytes, false);
        assert_eq!(kv(&d), pairs(&[("TITLE", "t"), ("COPYRIGHT", "odd")]));
        assert_eq!(d.streams()[0].frames, Some(frames as u64));
        assert_eq!(all_data(&mut d), data, "{format:?}");
    }
}

#[test]
fn create_muxer_hands_the_tags_to_the_wav_writer() {
    let s = stream(SampleFormat::S16, 1, 8000);
    let sink = Shared::default();
    let mut m = crate::create_muxer(
        crate::FormatId::Wav,
        Box::new(sink.clone()),
        &s,
        &tags(&[("TITLE", "via create_muxer")]),
    )
    .unwrap();
    m.finish().unwrap();
    let bytes = sink.0.lock().unwrap().get_ref().clone();
    let d = open(bytes, true);
    assert_eq!(kv(&d), pairs(&[("TITLE", "via create_muxer")]));
}
