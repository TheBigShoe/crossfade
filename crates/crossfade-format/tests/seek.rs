//! `Demuxer::seek` in every reader that can seek: where the next packet starts, what it holds,
//! the ends of the stream, and sources that cannot seek.

mod seek_support;

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Demuxer, Error, Limits, Packet, SampleFormat, StreamInfo,
    Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_testkit::io::FaultySource;
use seek_support::{Shared, drain, open_bytes};
use std::io::Cursor;

fn pcm_stream(codec: CodecId, channels: u16) -> StreamInfo {
    let layout = ChannelLayout::default_for(channels).unwrap();
    let audio = AudioSpec::new(8000, SampleFormat::S16, layout).unwrap();
    StreamInfo::new(0, codec, audio, TimeBase::new(1, 8000).unwrap())
}

/// A PCM file whose sample bytes count up, so any byte says where it sits.
fn pcm_file(format: FormatId, codec: CodecId, channels: u16, frames: usize) -> (Vec<u8>, Vec<u8>) {
    let width = 2 * usize::from(channels);
    let audio: Vec<u8> = (0..frames * width).map(|i| (i % 251) as u8).collect();
    let sink = Shared::default();
    let mut tags = Tags::new();
    tags.push("TITLE", "after the audio in WAV");
    let stream = pcm_stream(codec, channels);
    let mut m = create_muxer(format, Box::new(sink.clone()), &stream, &tags).unwrap();
    for chunk in audio.chunks(1000 * width) {
        m.write_packet(&Packet {
            data: chunk.to_vec(),
            duration: (chunk.len() / width) as u64,
            ..Packet::default()
        })
        .unwrap();
    }
    m.finish().unwrap();
    (sink.bytes(), audio)
}

fn pcm_cases() -> Vec<(&'static str, Vec<u8>, Vec<u8>, usize)> {
    let mut out = Vec::new();
    for (name, format, codec) in [
        ("wav", FormatId::Wav, CodecId::PcmS16Le),
        ("aiff", FormatId::Aiff, CodecId::PcmS16Be),
    ] {
        for channels in [1u16, 2, 6] {
            let (file, audio) = pcm_file(format, codec, channels, 10_007);
            out.push((name, file, audio, 2 * usize::from(channels)));
        }
    }
    out
}

/// The bytes of every packet from here on, checking the pts run on without a gap.
fn rest(d: &mut dyn Demuxer, mut pts: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for p in drain(d).unwrap() {
        assert_eq!(p.pts, pts);
        pts += p.duration as i64;
        out.extend(p.data);
    }
    out
}

#[test]
fn pcm_seeks_land_exactly_on_the_frame_asked_for() {
    for (name, file, audio, width) in pcm_cases() {
        let total = (audio.len() / width) as i64;
        let mut d = open_bytes(&file);
        assert_eq!(d.streams()[0].frames, Some(total as u64), "{name}");
        // Forwards, backwards, twice the same, the first and the last frame.
        for target in [0, 1, 4095, 4096, 5000, 17, 17, total - 1, 3, 0] {
            assert_eq!(d.seek(target).unwrap(), target, "{name} {target}");
            let got = rest(d.as_mut(), target);
            assert_eq!(got, audio[target as usize * width..], "{name} {target}");
        }
    }
}

#[test]
fn pcm_seeks_clamp_to_the_start_and_the_end() {
    for (name, file, audio, width) in pcm_cases() {
        let total = (audio.len() / width) as i64;
        let mut d = open_bytes(&file);
        for past in [total, total + 1, i64::MAX] {
            assert_eq!(d.seek(past).unwrap(), total, "{name} {past}");
            assert!(!d.read_packet(&mut Packet::default()).unwrap(), "{name}");
        }
        for before in [-1, i64::MIN] {
            assert_eq!(d.seek(before).unwrap(), 0, "{name} {before}");
            assert_eq!(rest(d.as_mut(), 0), audio, "{name}");
        }
    }
}

#[test]
fn seeking_in_the_middle_of_a_read_works_after_reading_to_the_end_too() {
    for (name, file, audio, width) in pcm_cases() {
        let mut d = open_bytes(&file);
        let mut p = Packet::default();
        assert!(d.read_packet(&mut p).unwrap());
        assert_eq!(d.seek(100).unwrap(), 100);
        assert_eq!(rest(d.as_mut(), 100), audio[100 * width..], "{name}");
        assert_eq!(d.seek(9000).unwrap(), 9000);
        assert_eq!(rest(d.as_mut(), 9000), audio[9000 * width..], "{name}");
    }
}

#[test]
fn a_reader_on_a_pipe_cannot_seek() {
    for (name, file, ..) in pcm_cases() {
        let pipe = FaultySource::new(Cursor::new(file)).not_seekable();
        let (_, mut d) = open_demuxer(Box::new(pipe), &Limits::default()).unwrap();
        assert!(
            matches!(d.seek(10), Err(Error::Unsupported { .. })),
            "{name}"
        );
    }
}

#[test]
fn a_streaming_wav_ends_where_its_source_ends() {
    let (mut file, audio) = pcm_file(FormatId::Wav, CodecId::PcmS16Le, 2, 3000);
    // No tags after the audio: the data chunk runs to the end of the file.
    let at = file.windows(4).position(|w| w == b"data").unwrap();
    file.truncate(at + 8 + audio.len());
    file[at + 4..at + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut d = open_bytes(&file);
    assert_eq!(d.streams()[0].frames, None);
    assert_eq!(d.seek(2999).unwrap(), 2999);
    assert_eq!(rest(d.as_mut(), 2999), audio[2999 * 4..]);
    assert_eq!(d.seek(5000).unwrap(), 3000);
    assert!(!d.read_packet(&mut Packet::default()).unwrap());
    assert_eq!(d.seek(1).unwrap(), 1);
    assert_eq!(rest(d.as_mut(), 1), audio[4..]);
}
