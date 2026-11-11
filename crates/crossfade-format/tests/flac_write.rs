//! Encoder and writer together: whole-file invariants checked by a reader built only from
//! the shared parsers, with no decoder.

use crossfade_codec::{EncodeOptions, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Limits, MediaSink, Packet, SampleFormat,
    Samples, StreamInfo, Tags, TimeBase,
};
use crossfade_flac::crc::crc16;
use crossfade_flac::frame::parse_header;
use crossfade_flac::md5::Md5;
use crossfade_flac::streaminfo::StreamInfoBlock;
use crossfade_format::vorbis_comment;
use crossfade_format::{FormatId, create_muxer};
use std::io::{Cursor, SeekFrom};
use std::sync::{Arc, Mutex};

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
        _ => Samples::S24(samples.to_vec()),
    };
    f
}

/// What the pipeline does: provisional header into the writer, final one after the last packet.
fn write_file(spec: &AudioSpec, samples: &[i32], tags: &Tags, level: Option<u8>) -> Vec<u8> {
    let mut enc = encoder_for(
        CodecId::Flac,
        spec,
        &EncodeOptions {
            compression_level: level,
            ..EncodeOptions::default()
        },
    )
    .unwrap();
    let mut info = StreamInfo::new(
        0,
        CodecId::Flac,
        spec.clone(),
        TimeBase::new(1, i64::from(spec.sample_rate())).unwrap(),
    );
    info.extradata = enc.extradata().to_vec();
    let sink = Shared::default();
    let mut mux = create_muxer(FormatId::Flac, Box::new(sink.clone()), &info, tags).unwrap();
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
    mux.set_extradata(enc.extradata()).unwrap();
    mux.finish().unwrap();
    sink.0.lock().unwrap().get_ref().clone()
}

struct Walked {
    info: StreamInfoBlock,
    tags: Tags,
    /// Byte length and block size of each frame.
    frames: Vec<(usize, u32)>,
}

/// Splits the frames by finding the next header whose number follows and whose predecessor's
/// CRC-16 closes, so it does not trust the encoder's packet boundaries.
fn walk(file: &[u8]) -> Walked {
    assert_eq!(&file[..4], b"fLaC");
    let mut at = 4;
    let mut info = None;
    let mut tags = Tags::new();
    loop {
        let flag = file[at] & 0x80 != 0;
        let kind = file[at] & 0x7F;
        let len = usize::from(file[at + 1]) << 16
            | usize::from(file[at + 2]) << 8
            | usize::from(file[at + 3]);
        let body = &file[at + 4..at + 4 + len];
        match kind {
            0 => info = Some(StreamInfoBlock::parse(body).unwrap()),
            4 => {
                tags = vorbis_comment::parse(body, &Limits::default())
                    .unwrap()
                    .tags;
            }
            _ => {}
        }
        at += 4 + len;
        if flag {
            break;
        }
    }
    let info = info.expect("STREAMINFO comes first");
    let mut frames = Vec::new();
    while at < file.len() {
        let head = parse_header(&file[at..]).unwrap();
        assert_eq!(head.number, frames.len() as u64);
        let end = (at + head.header_len + 1..=file.len())
            .find(|&e| {
                let closes = crc16(&file[at..e - 2]).to_be_bytes() == file[e - 2..e];
                closes
                    && (e == file.len()
                        || parse_header(&file[e..]).is_ok_and(|n| n.number == head.number + 1))
            })
            .expect("frame closes");
        frames.push((end - at, head.block_size));
        at = end;
    }
    Walked { info, tags, frames }
}

fn rfc_md5(bytes_per_sample: usize, samples: &[i32]) -> [u8; 16] {
    let mut m = Md5::new();
    for s in samples {
        m.update(&s.to_le_bytes()[..bytes_per_sample]);
    }
    m.finalize()
}

fn check(rate: u32, format: SampleFormat, channels: u16, frames: usize, level: Option<u8>) {
    let spec =
        AudioSpec::new(rate, format, ChannelLayout::unpositioned(channels).unwrap()).unwrap();
    let bits = match format {
        SampleFormat::U8 => 8,
        SampleFormat::S16 => 16,
        _ => 24,
    };
    let samples = tone(usize::from(channels), frames, bits);
    let mut tags = Tags::new();
    tags.push("TITLE", "Tone");
    tags.push("ARTIST", "Crossfade");
    let file = write_file(&spec, &samples, &tags, level);
    let w = walk(&file);

    assert_eq!(w.tags, tags);
    assert_eq!(w.info.total_samples, frames as u64);
    assert_eq!(w.info.md5, rfc_md5(bits as usize / 8, &samples));
    assert_eq!(w.info.sample_rate, rate);
    assert_eq!(u16::from(w.info.channels), channels);
    assert_eq!(u32::from(w.info.bits_per_sample), bits);
    assert_eq!(w.info.min_block_size, w.info.max_block_size);
    let block = u32::from(w.info.max_block_size);
    let sum: u64 = w.frames.iter().map(|f| u64::from(f.1)).sum();
    assert_eq!(sum, frames as u64);
    assert!(w.frames.iter().all(|f| f.1 == block) || w.frames.last().is_some_and(|l| l.1 < block));
    if let Some((_, last)) = w.frames.split_last() {
        assert!(last.iter().all(|f| f.1 == block));
    }
    let sizes = w.frames.iter().map(|f| f.0 as u32);
    assert_eq!(w.info.min_frame_size, sizes.clone().min().unwrap_or(0));
    assert_eq!(w.info.max_frame_size, sizes.max().unwrap_or(0));
}

#[test]
fn complete_files_satisfy_the_whole_file_invariants() {
    check(44100, SampleFormat::S16, 2, 10_000, None);
    check(8000, SampleFormat::U8, 1, 5_000, Some(0));
    check(96000, SampleFormat::S24, 6, 4_500, Some(8));
    check(44100, SampleFormat::S16, 2, 4096 * 3, Some(3));
}

#[test]
fn a_short_stream_and_an_empty_one_are_complete_files() {
    check(44100, SampleFormat::S16, 1, 10, None);
    check(44100, SampleFormat::S16, 2, 0, None);
}
