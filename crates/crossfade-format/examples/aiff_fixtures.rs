//! Writes the inputs for the AIFF fixtures and, for checking against Apple's tools, our own
//! AIFF and tagged WAV output.
//!
//!   cargo run -p crossfade-format --example aiff_fixtures -- sources DIR
//!   cargo run -p crossfade-format --example aiff_fixtures -- ours DIR

#[path = "../tests/support/signal.rs"]
mod signal;

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, MediaSink, Packet, SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer};
use signal::RATE;
use std::fs::File;

const FORMATS: [(&str, SampleFormat); 6] = [
    ("u8", SampleFormat::U8),
    ("s16", SampleFormat::S16),
    ("s24", SampleFormat::S24),
    ("s32", SampleFormat::S32),
    ("f32", SampleFormat::F32),
    ("f64", SampleFormat::F64),
];

fn write_file(
    path: &str,
    format: FormatId,
    codec: CodecId,
    sample: SampleFormat,
    data: Vec<u8>,
    tags: &Tags,
) {
    let audio = AudioSpec::new(RATE, sample, ChannelLayout::stereo()).unwrap();
    let info = StreamInfo::new(0, codec, audio, TimeBase::new(1, i64::from(RATE)).unwrap());
    let sink: Box<dyn MediaSink> = Box::new(File::create(path).unwrap());
    let mut m = create_muxer(format, sink, &info, tags).unwrap();
    m.write_packet(&Packet {
        data,
        ..Packet::default()
    })
    .unwrap();
    m.finish().unwrap();
}

/// Big-endian bytes of little-endian `le`; signed 8-bit flips the offset.
fn to_be(le: &[u8], sample: SampleFormat) -> Vec<u8> {
    match sample {
        SampleFormat::U8 => le.iter().map(|b| b ^ 0x80).collect(),
        _ => le
            .chunks_exact(sample.bytes_per_sample())
            .flat_map(|c| c.iter().rev().copied())
            .collect(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (mode, dir) = (args[1].as_str(), args[2].as_str());
    std::fs::create_dir_all(dir).unwrap();
    let mut tags = Tags::new();
    tags.push("TITLE", "Fixture title");
    tags.push("ARTIST", "Fixture artist");
    tags.push("COMMENT", "one");
    tags.push("COMMENT", "two");
    match mode {
        "sources" => {
            for (name, sample) in FORMATS {
                let le = signal::le_bytes(sample);
                let path = format!("{dir}/{name}.wav");
                write_file(
                    &path,
                    FormatId::Wav,
                    CodecId::pcm_for(sample),
                    sample,
                    le,
                    &Tags::new(),
                );
            }
            // afconvert copies an unsigned 8-bit source to I8 without converting it, so the
            // signed 8-bit fixtures come from 16-bit samples that are exactly 8-bit values.
            let wide: Vec<u8> = signal::le_bytes(SampleFormat::U8)
                .iter()
                .flat_map(|&u| (i16::from(u) - 128).wrapping_mul(256).to_le_bytes())
                .collect();
            write_file(
                &format!("{dir}/s8src.wav"),
                FormatId::Wav,
                CodecId::PcmS16Le,
                SampleFormat::S16,
                wide,
                &Tags::new(),
            );
        }
        "ours" => {
            for (name, sample) in FORMATS {
                let le = signal::le_bytes(sample);
                let tagged = format!("{dir}/{name}-tagged.wav");
                write_file(
                    &tagged,
                    FormatId::Wav,
                    CodecId::pcm_for(sample),
                    sample,
                    le.clone(),
                    &tags,
                );
                let be_codec = CodecId::pcm_be_for(sample);
                let path = format!("{dir}/{name}.aiff");
                write_file(
                    &path,
                    FormatId::Aiff,
                    be_codec,
                    sample,
                    to_be(&le, sample),
                    &tags,
                );
            }
        }
        _ => panic!("mode is sources or ours"),
    }
}
