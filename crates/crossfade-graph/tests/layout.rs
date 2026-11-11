//! A speaker layout survives conversion between WAV and FLAC (the channel mask), with the
//! channels left in the order they are stored. Files are built here, not by an external tool.
mod common;

use common::job;
use crossfade_codec::{EncodeOptions, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, JobIo, Limits, Packet, SampleFormat, Samples,
    StreamInfo, Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_graph::{CancelToken, run};
use crossfade_testkit::memory::MemoryIo;
use std::io::Cursor;

const KEY: &[u8] = b"WAVEFORMATEXTENSIBLE_CHANNEL_MASK=";

/// A 16-bit extensible WAV with `mask`, whose samples are 1, 2, 3, ... so order shows.
fn wav(channels: u16, mask: u32) -> Vec<u8> {
    let frames = 50usize;
    let mut fmt = Vec::new();
    for (v, n) in [
        (0xFFFEu32, 2),
        (u32::from(channels), 2),
        (48_000, 4),
        (48_000 * 2 * u32::from(channels), 4),
        (2 * u32::from(channels), 2),
        (16, 2),
        (22, 2),
        (16, 2),
        (mask, 4),
        (1, 2),
    ] {
        fmt.extend_from_slice(&v.to_le_bytes()[..n]);
    }
    fmt.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    let pcm: Vec<u8> = (0..frames * usize::from(channels))
        .flat_map(|i| (i as i16 + 1).to_le_bytes())
        .collect();
    let mut body = b"WAVEfmt ".to_vec();
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    body.extend_from_slice(&pcm);
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// The `dwChannelMask` of an extensible WAV, or `None` for a plain header.
fn wav_mask(bytes: &[u8]) -> Option<u32> {
    assert_eq!(&bytes[12..16], b"fmt ");
    (bytes[20..22] == [0xFE, 0xFF]).then(|| u32::from_le_bytes(bytes[40..44].try_into().unwrap()))
}

fn data(bytes: &[u8]) -> &[u8] {
    let at = bytes.windows(4).position(|w| w == b"data").unwrap();
    &bytes[at + 8..]
}

fn mask_tag(flac: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = flac;
    while let Some(at) = rest.windows(KEY.len()).position(|w| w == KEY) {
        let value = &rest[at + KEY.len()..];
        let end = value
            .iter()
            .position(|&b| !b.is_ascii_alphanumeric())
            .unwrap();
        found.push(String::from_utf8(value[..end].to_vec()).unwrap());
        rest = &rest[at + KEY.len()..];
    }
    found
}

fn convert(io: &mut MemoryIo, from: &str, to: &str) {
    run(
        &job(from, to, None),
        io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
}

fn via_flac(mask: u32, channels: u16) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let source = wav(channels, mask);
    let mut io = MemoryIo::new();
    io.insert("in.wav", source.clone());
    convert(&mut io, "in.wav", "mid.flac");
    convert(&mut io, "mid.flac", "out.wav");
    (
        source,
        io.get("mid.flac").unwrap(),
        io.get("out.wav").unwrap(),
    )
}

#[test]
fn a_non_default_mask_survives_wav_flac_wav_and_is_tagged() {
    // 5.1 with back channels (0x3F), and the 5.1 side layout of the issue's file.
    for (mask, channels, tag) in [
        (0x3Fu32, 6u16, Some("0x003F")),
        (0x60F, 6, None),
        (0x1, 1, Some("0x0001")),
        (0x3F003, 8, Some("0x3F003")),
        (0x607, 5, None),
        (0x70F, 7, None),
        (0x63F, 8, None),
    ] {
        let (source, flac, out) = via_flac(mask, channels);
        assert_eq!(wav_mask(&out), Some(mask), "{mask:#x}");
        assert_eq!(
            mask_tag(&flac),
            tag.map(String::from).into_iter().collect::<Vec<_>>(),
            "{mask:#x}"
        );
        // Channels are never reordered.
        assert_eq!(data(&out), data(&source), "{mask:#x}");
    }
}

/// A 6-channel FLAC made with the encoder and the muxer; the muxer writes the mask tag when
/// `layout` is not the default.
fn flac_with(layout: ChannelLayout) -> Vec<u8> {
    let audio = AudioSpec::new(48000, SampleFormat::S16, layout).unwrap();
    let mut enc = encoder_for(CodecId::Flac, &audio, &EncodeOptions::default()).unwrap();
    let mut frame = AudioFrame::new(&audio);
    frame.samples = Samples::S16((0..6 * 100).map(|i| i as i16 + 1).collect());
    let stream = StreamInfo {
        frames: Some(100),
        ..StreamInfo::new(0, CodecId::Flac, audio, TimeBase::new(1, 48000).unwrap())
    };
    let mut io = MemoryIo::new();
    let sink = io.create_sink("t.flac").unwrap();
    let mut mux = create_muxer(FormatId::Flac, sink, &stream, &Tags::new()).unwrap();
    enc.send_frame(&frame).unwrap();
    enc.drain();
    let mut p = Packet::default();
    while enc.receive_packet(&mut p).unwrap() {
        mux.write_packet(&p).unwrap();
    }
    mux.set_extradata(enc.extradata()).unwrap();
    mux.finish().unwrap();
    io.commit("t.flac").unwrap();
    io.get("t.flac").unwrap()
}

#[test]
fn a_flac_with_the_tag_gives_its_mask_to_the_wav() {
    // 0x3F is tagged; the side layout, the default, is not, and still reads as 0x60F.
    for (mask, tagged) in [(0x3Fu32, true), (0x60F, false)] {
        let flac = flac_with(ChannelLayout::from_mask(mask, 6).unwrap());
        assert_eq!(!mask_tag(&flac).is_empty(), tagged, "{mask:#x}");
        let mut io = MemoryIo::new();
        io.insert("in.flac", flac);
        convert(&mut io, "in.flac", "out.wav");
        let out = io.get("out.wav").unwrap();
        assert_eq!(wav_mask(&out), Some(mask));
        let want: Vec<u8> = (0..600).flat_map(|i: i16| (i + 1).to_le_bytes()).collect();
        assert_eq!(data(&out), want.as_slice());
    }
}

#[test]
fn an_invalid_tag_is_ignored() {
    let mut flac = flac_with(ChannelLayout::from_mask(0x3F, 6).unwrap());
    // Same length, so no block size changes: five bits for six channels.
    let at = flac.windows(KEY.len()).position(|w| w == KEY).unwrap() + KEY.len();
    flac[at..at + 6].copy_from_slice(b"0x0007");
    let mut io = MemoryIo::new();
    io.insert("in.flac", flac);
    convert(&mut io, "in.flac", "out.wav");
    assert_eq!(wav_mask(&io.get("out.wav").unwrap()), Some(0x60F));
}

#[test]
fn a_flac_stream_description_carries_the_layout() {
    let flac = flac_with(ChannelLayout::from_mask(0x3F, 6).unwrap());
    let (_, d) = open_demuxer(Box::new(Cursor::new(flac)), &Limits::default()).unwrap();
    assert_eq!(d.streams()[0].audio.layout().to_mask(), Some(0x3F));
}
