//! The Ogg FLAC writer driven the way the pipeline drives it: encoder, muxer, then our own
//! reader and decoder to read the file back.

use crossfade_codec::{EncodeOptions, decoder_for, encoder_for_threads};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Error, Limits, MediaSink, Muxer, Packet,
    SampleFormat, Samples, StreamInfo, Tags, TimeBase,
};
use crossfade_flac::streaminfo::StreamInfoBlock;
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_testkit::io::FaultySink;
use std::io::{Cursor, ErrorKind, SeekFrom};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Shared {
    bytes: Arc<Mutex<Cursor<Vec<u8>>>>,
    seekable: bool,
}

impl Shared {
    fn new(seekable: bool) -> Self {
        Self {
            bytes: Arc::default(),
            seekable,
        }
    }
    fn contents(&self) -> Vec<u8> {
        self.bytes.lock().unwrap().get_ref().clone()
    }
}

impl MediaSink for Shared {
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(&mut *self.bytes.lock().unwrap(), buf)
    }
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        if !self.seekable {
            return Err(ErrorKind::Unsupported.into());
        }
        std::io::Seek::seek(&mut *self.bytes.lock().unwrap(), pos)
    }
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Page {
    flags: u8,
    granule: i64,
    serial: u32,
    sequence: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    bytes: Vec<u8>,
    crc_ok: bool,
}

fn crc32(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |mut c, &b| {
        c ^= u32::from(b) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04C1_1DB7
            } else {
                c << 1
            };
        }
        c
    })
}

fn pages(file: &[u8]) -> Vec<Page> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < file.len() {
        let h = &file[at..];
        assert_eq!(&h[..4], b"OggS");
        let n = usize::from(h[26]);
        let lacing = h[27..27 + n].to_vec();
        let len: usize = lacing.iter().map(|&l| usize::from(l)).sum();
        let total = 27 + n + len;
        let mut zeroed = h[..total].to_vec();
        zeroed[22..26].fill(0);
        out.push(Page {
            flags: h[5],
            granule: i64::from_le_bytes(h[6..14].try_into().unwrap()),
            serial: u32::from_le_bytes(h[14..18].try_into().unwrap()),
            sequence: u32::from_le_bytes(h[18..22].try_into().unwrap()),
            lacing,
            body: h[27 + n..total].to_vec(),
            bytes: h[..total].to_vec(),
            crc_ok: crc32(&zeroed) == u32::from_le_bytes(h[22..26].try_into().unwrap()),
        });
        at += total;
    }
    out
}

fn tone(channels: usize, frames: usize, bits: u32) -> Vec<i32> {
    let amp = f64::from((1u32 << (bits - 1)) - 1) * 0.4;
    (0..frames * channels)
        .map(|i| {
            let (t, c) = (i / channels, i % channels);
            ((t as f64 * (0.05 + 0.02 * c as f64)).sin() * amp) as i32
        })
        .collect()
}

fn frame(spec: &AudioSpec, samples: &[i32]) -> AudioFrame {
    let mut f = AudioFrame::new(spec);
    f.samples = match spec.format() {
        SampleFormat::U8 => Samples::U8(samples.iter().map(|&v| (v + 128) as u8).collect()),
        SampleFormat::S16 => Samples::S16(samples.iter().map(|&v| v as i16).collect()),
        SampleFormat::S24 => Samples::S24(samples.to_vec()),
        _ => Samples::S32(samples.to_vec()),
    };
    f
}

fn flatten(s: &Samples) -> Vec<i32> {
    match s {
        Samples::U8(v) => v.iter().map(|&x| i32::from(x) - 128).collect(),
        Samples::S16(v) => v.iter().map(|&x| i32::from(x)).collect(),
        Samples::S24(v) | Samples::S32(v) => v.clone(),
        _ => unreachable!(),
    }
}

fn spec(rate: u32, format: SampleFormat, channels: u16) -> AudioSpec {
    AudioSpec::new(rate, format, ChannelLayout::unpositioned(channels).unwrap()).unwrap()
}

fn bits_of(format: SampleFormat) -> u32 {
    match format {
        SampleFormat::U8 => 8,
        SampleFormat::S16 => 16,
        SampleFormat::S24 => 24,
        _ => 32,
    }
}

struct Written {
    file: Vec<u8>,
    /// What the encoder said at the start and what it said at the end.
    provisional: Vec<u8>,
    last: Vec<u8>,
}

fn write(
    spec: &AudioSpec,
    samples: &[i32],
    tags: &Tags,
    seekable: bool,
    threads: usize,
) -> Written {
    let mut enc =
        encoder_for_threads(CodecId::Flac, spec, &EncodeOptions::default(), threads).unwrap();
    let mut info = StreamInfo::new(
        0,
        CodecId::Flac,
        spec.clone(),
        TimeBase::new(1, i64::from(spec.sample_rate())).unwrap(),
    );
    info.extradata = enc.extradata().to_vec();
    let provisional = info.extradata.clone();
    let sink = Shared::new(seekable);
    let mut mux = create_muxer(FormatId::Ogg, Box::new(sink.clone()), &info, tags).unwrap();
    let per = 3000 * usize::from(spec.layout().channels());
    let mut p = Packet::default();
    for piece in samples.chunks(per) {
        enc.send_frame(&frame(spec, piece)).unwrap();
        while enc.receive_packet(&mut p).unwrap() {
            mux.write_packet(&p).unwrap();
        }
    }
    enc.drain();
    while enc.receive_packet(&mut p).unwrap() {
        mux.write_packet(&p).unwrap();
    }
    let last = enc.extradata().to_vec();
    mux.set_extradata(&last).unwrap();
    mux.finish().unwrap();
    Written {
        file: sink.contents(),
        provisional,
        last,
    }
}

fn read_back(file: &[u8]) -> (StreamInfo, Tags, Vec<Packet>) {
    let (format, mut d) =
        open_demuxer(Box::new(Cursor::new(file.to_vec())), &Limits::default()).unwrap();
    assert_eq!(format, FormatId::Ogg);
    let mut packets = Vec::new();
    loop {
        let mut p = Packet::default();
        if !d.read_packet(&mut p).unwrap() {
            break;
        }
        packets.push(p);
    }
    (d.streams()[0].clone(), d.tags().clone(), packets)
}

fn decode(stream: &StreamInfo, packets: &[Packet]) -> Vec<i32> {
    let mut dec = decoder_for(stream).unwrap();
    let mut out = Vec::new();
    let mut f = AudioFrame::new(&stream.audio);
    for p in packets {
        dec.send_packet(p).unwrap();
        while dec.receive_frame(&mut f).unwrap() {
            out.extend(flatten(&f.samples));
        }
    }
    dec.drain();
    while dec.receive_frame(&mut f).unwrap() {
        out.extend(flatten(&f.samples));
    }
    out
}

fn tags() -> Tags {
    let mut t = Tags::new();
    t.push("TITLE", "Tone");
    t.push("ARTIST", "Crossfade");
    t
}

#[test]
fn the_file_is_headers_then_audio_with_exact_granules() {
    let s = spec(44100, SampleFormat::S16, 2);
    let w = write(&s, &tone(2, 20_000, 16), &tags(), true, 1);
    let pages = pages(&w.file);

    // Identification page: alone, BOS, granule 0, exactly 27 + 1 + 51 bytes.
    assert_eq!(pages[0].bytes.len(), 79);
    assert_eq!(pages[0].flags, 0x02);
    assert_eq!(pages[0].granule, 0);
    assert_eq!(pages[0].lacing, [51]);
    let id = &pages[0].body;
    assert_eq!(&id[..9], b"\x7fFLAC\x01\x00\x00\x01");
    assert_eq!(&id[9..13], b"fLaC");
    assert_eq!(&id[13..17], [0, 0, 0, 34]);

    // One more header packet (the comment, last-block flagged), on a page of its own.
    assert_eq!(pages[1].granule, 0);
    assert_eq!(pages[1].flags, 0);
    assert_eq!(pages[1].body[0], 0x84);
    assert_eq!(pages[1].lacing.iter().filter(|&&l| l < 255).count(), 1);

    // Audio from page 2: sample counts through the last packet that ends on the page.
    let audio = &pages[2..];
    assert!(!audio.is_empty());
    let (_, _, packets) = read_back(&w.file);
    let mut done = 0u64;
    let mut seen = 0usize;
    for (i, p) in audio.iter().enumerate() {
        let ends = p.lacing.iter().filter(|&&l| l < 255).count();
        for q in &packets[seen..seen + ends] {
            done += q.duration;
        }
        seen += ends;
        assert_eq!(p.granule, if ends == 0 { -1 } else { done as i64 });
        assert_eq!(p.flags & 0x04 != 0, i + 1 == audio.len());
    }
    assert_eq!(seen, packets.len());
    assert_eq!(done, 20_000);

    let serial = pages[0].serial;
    for (i, p) in pages.iter().enumerate() {
        assert_eq!((p.sequence, p.serial, p.crc_ok), (i as u32, serial, true));
    }
}

#[test]
fn the_final_streaminfo_replaces_the_provisional_one_on_a_seekable_sink() {
    let s = spec(44100, SampleFormat::S16, 2);
    let w = write(&s, &tone(2, 20_000, 16), &tags(), true, 1);
    let first = &pages(&w.file)[0];
    assert!(first.crc_ok);
    assert_eq!(first.body[17..], w.last[..]);
    let info = StreamInfoBlock::parse(&first.body[17..]).unwrap();
    assert_eq!(info.total_samples, 20_000);
    assert!(info.min_frame_size > 0 && info.max_frame_size >= info.min_frame_size);
    assert_ne!(info.md5, [0; 16]);
    assert_ne!(w.last, w.provisional);
}

#[test]
fn a_sink_that_cannot_seek_keeps_the_provisional_streaminfo_and_a_valid_file() {
    let s = spec(44100, SampleFormat::S16, 2);
    let samples = tone(2, 20_000, 16);
    let w = write(&s, &samples, &tags(), false, 1);
    let first = &pages(&w.file)[0];
    assert!(first.crc_ok);
    assert_eq!(first.body[17..], w.provisional[..]);
    assert!(pages(&w.file).iter().all(|p| p.crc_ok));
    let (stream, _, packets) = read_back(&w.file);
    assert_eq!(decode(&stream, &packets), samples);
}

#[test]
fn the_same_input_gives_the_same_bytes_at_any_thread_count() {
    let s = spec(48000, SampleFormat::S16, 2);
    let samples = tone(2, 50_000, 16);
    let one = write(&s, &samples, &tags(), true, 1).file;
    assert_eq!(write(&s, &samples, &tags(), true, 1).file, one);
    for threads in [0, 3] {
        assert_eq!(write(&s, &samples, &tags(), true, threads).file, one);
    }
}

#[test]
fn every_depth_and_channel_count_round_trips_bit_exact_with_tags() {
    for (format, rate) in [
        (SampleFormat::U8, 8000),
        (SampleFormat::S16, 44100),
        (SampleFormat::S24, 96000),
        (SampleFormat::S32, 48000),
    ] {
        for channels in 1..=8u16 {
            let s = spec(rate, format, channels);
            let samples = tone(usize::from(channels), 9_000, bits_of(format));
            let w = write(&s, &samples, &tags(), true, 1);
            let (stream, got_tags, packets) = read_back(&w.file);
            assert_eq!(got_tags, tags(), "{format:?} x{channels}");
            assert_eq!(stream.frames, Some(9_000));
            assert_eq!(
                decode(&stream, &packets),
                samples,
                "{format:?} x{channels}"
            );
        }
    }
}

#[test]
fn an_empty_stream_is_a_valid_file_with_no_packets() {
    let s = spec(44100, SampleFormat::S16, 2);
    let w = write(&s, &[], &Tags::new(), true, 1);
    let p = pages(&w.file);
    assert!(p.iter().all(|p| p.crc_ok));
    assert_eq!(p.last().unwrap().flags & 0x04, 0x04);
    assert!(read_back(&w.file).2.is_empty());
}

fn native_flac(samples: &[i32], s: &AudioSpec) -> Vec<u8> {
    let mut enc = encoder_for_threads(CodecId::Flac, s, &EncodeOptions::default(), 1).unwrap();
    let mut info = StreamInfo::new(
        0,
        CodecId::Flac,
        s.clone(),
        TimeBase::new(1, i64::from(s.sample_rate())).unwrap(),
    );
    info.extradata = enc.extradata().to_vec();
    let sink = Shared::new(true);
    let mut mux = create_muxer(FormatId::Flac, Box::new(sink.clone()), &info, &tags()).unwrap();
    enc.send_frame(&frame(s, samples)).unwrap();
    enc.drain();
    let mut p = Packet::default();
    while enc.receive_packet(&mut p).unwrap() {
        mux.write_packet(&p).unwrap();
    }
    mux.set_extradata(enc.extradata()).unwrap();
    mux.finish().unwrap();
    sink.contents()
}

/// Stream copy: demux `input`, hand its packets and STREAMINFO to a muxer for `to`.
fn copy(input: &[u8], to: FormatId) -> Vec<u8> {
    let (_, mut d) = open_demuxer(Box::new(Cursor::new(input.to_vec())), &Limits::default()).unwrap();
    let sink = Shared::new(true);
    let stream = d.streams()[0].clone();
    let mut mux = create_muxer(to, Box::new(sink.clone()), &stream, d.tags()).unwrap();
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        mux.write_packet(&p).unwrap();
    }
    mux.finish().unwrap();
    sink.contents()
}

#[test]
fn native_flac_copied_into_ogg_and_back_is_the_same_file() {
    let s = spec(44100, SampleFormat::S16, 2);
    let native = native_flac(&tone(2, 30_000, 16), &s);
    let ogg = copy(&native, FormatId::Ogg);
    assert_eq!(&ogg[..4], b"OggS");
    let (_, got_tags, ogg_packets) = read_back(&ogg);
    assert_eq!(got_tags, tags());
    let (_, _, native_packets) = read_back_native(&native);
    assert_eq!(
        ogg_packets.iter().map(|p| &p.data).collect::<Vec<_>>(),
        native_packets.iter().map(|p| &p.data).collect::<Vec<_>>()
    );
    // The source's STREAMINFO (total, sizes, MD5) rides along unchanged.
    assert_eq!(pages(&ogg)[0].body[17..], native[8..42]);
    assert_eq!(copy(&ogg, FormatId::Flac), native);
}

fn read_back_native(file: &[u8]) -> (StreamInfo, Tags, Vec<Packet>) {
    let (_, mut d) = open_demuxer(Box::new(Cursor::new(file.to_vec())), &Limits::default()).unwrap();
    let mut packets = Vec::new();
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    (d.streams()[0].clone(), d.tags().clone(), packets)
}

#[test]
fn ogg_holds_flac_and_nothing_else() {
    let s = spec(44100, SampleFormat::S16, 2);
    let info = StreamInfo::new(0, CodecId::PcmS16Le, s, TimeBase::new(1, 44100).unwrap());
    let err = create_muxer(
        FormatId::Ogg,
        Box::new(Shared::new(true)),
        &info,
        &Tags::new(),
    )
    .map(|_| ())
    .unwrap_err();
    assert!(
        matches!(&err, Error::Unsupported { what } if what.contains("flac")),
        "{err:?}"
    );
}

#[test]
fn a_sink_that_fails_surfaces_the_error_wherever_it_fails() {
    let s = spec(44100, SampleFormat::S16, 2);
    let samples = tone(2, 20_000, 16);
    let mut enc = encoder_for_threads(CodecId::Flac, &s, &EncodeOptions::default(), 1).unwrap();
    let mut info = StreamInfo::new(0, CodecId::Flac, s.clone(), TimeBase::new(1, 44100).unwrap());
    info.extradata = enc.extradata().to_vec();
    enc.send_frame(&frame(&s, &samples)).unwrap();
    enc.drain();
    let mut packets = Vec::new();
    let mut p = Packet::default();
    while enc.receive_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    let total = write(&s, &samples, &Tags::new(), true, 1).file.len() as u64;
    // Fails in the headers, in the audio, and in the closing page.
    for limit in [10, 100, total / 2, total - 5] {
        let sink = FaultySink::new(Shared::new(true)).fail_after(limit, ErrorKind::StorageFull);
        let result = create_muxer(FormatId::Ogg, Box::new(sink), &info, &Tags::new()).and_then(
            |mut m| {
                for p in &packets {
                    m.write_packet(p)?;
                }
                m.set_extradata(enc.extradata())?;
                m.finish()
            },
        );
        assert!(result.is_err(), "limit {limit}");
    }
    let sink = FaultySink::new(Shared::new(true)).fail_on_finish(ErrorKind::Other);
    let mut m = create_muxer(FormatId::Ogg, Box::new(sink), &info, &Tags::new()).unwrap();
    for p in &packets {
        m.write_packet(p).unwrap();
    }
    assert!(m.finish().is_err());
}
