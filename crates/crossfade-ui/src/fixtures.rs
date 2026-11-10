//! Media files for tests and screenshots, written by the engine's own muxers and encoders
//! so a fixture is what a real conversion would produce. Compiled only for tests and for
//! the screenshots example (which includes this file by path).
#![allow(dead_code, unreachable_pub)]

use crossfade_core::{JobIo, Limits, Packet, SampleFormat, Tags};
use crossfade_flac::streaminfo::StreamInfoBlock;
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, run};
use crossfade_testkit::memory::MemoryIo;

/// A WAV of `frames` frames of a quiet 440 Hz sine, the same in every channel.
pub fn wav(rate: u32, channels: u16, format: SampleFormat, frames: u32) -> Vec<u8> {
    let width = format.bytes_per_sample();
    let data_len = frames * u32::from(channels) * u32::try_from(width).expect("small");
    let float = format.is_float();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&(if float { 3u16 } else { 1 }).to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * u32::from(channels) * width as u32).to_le_bytes());
    v.extend_from_slice(&(channels * width as u16).to_le_bytes());
    v.extend_from_slice(&((width * 8) as u16).to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let t = f64::from(i) / f64::from(rate);
        let x = 0.4 * (std::f64::consts::TAU * 440.0 * t).sin();
        for _ in 0..channels {
            match format {
                SampleFormat::S16 => v.extend_from_slice(&((x * 32767.0) as i16).to_le_bytes()),
                SampleFormat::S24 => {
                    // Multiples of 16, so the low four bits are free to be absent.
                    let q = ((x * 8_388_607.0) as i32) & !0xF;
                    v.extend_from_slice(&q.to_le_bytes()[..3]);
                }
                SampleFormat::S32 => {
                    v.extend_from_slice(&((x * 2_147_483_647.0) as i32).to_le_bytes());
                }
                SampleFormat::F32 => v.extend_from_slice(&(x as f32).to_le_bytes()),
                other => panic!("no fixture for {}", other.name()),
            }
        }
    }
    v
}

/// `source` converted by the engine to a file called `to` (its extension picks the
/// container).
pub fn convert(source: &[u8], to: &str, audio: AudioSettings) -> Vec<u8> {
    let mut io = MemoryIo::new();
    io.insert("fixture-in", source.to_vec());
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput {
            path: "fixture-in".into(),
        }],
        outputs: vec![JobOutput {
            path: to.into(),
            audio,
        }],
    };
    run(
        &job,
        &mut io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .expect("the engine converts the fixture");
    io.get(to).expect("the output was committed")
}

pub fn flac(rate: u32, channels: u16, format: SampleFormat, frames: u32) -> Vec<u8> {
    convert(
        &wav(rate, channels, format, frames),
        "fixture.flac",
        AudioSettings::default(),
    )
}

/// `flac` with `tags` in its Vorbis comment block, the audio packets untouched.
pub fn tagged(flac: &[u8], tags: &[(&str, &str)]) -> Vec<u8> {
    let mut io = MemoryIo::new();
    io.insert("in.flac", flac.to_vec());
    let source = io.open_source("in.flac").expect("open");
    let (_, mut demuxer) = open_demuxer(source, &Limits::default()).expect("a flac file");
    let stream = demuxer.streams()[0].clone();
    let mut all = Tags::new();
    for (key, value) in tags {
        all.push(*key, *value);
    }
    let sink = io.create_sink("out.flac").expect("sink");
    let mut muxer = create_muxer(FormatId::Flac, sink, &stream, &all).expect("muxer");
    let mut packet = Packet::default();
    while demuxer.read_packet(&mut packet).expect("packet") {
        muxer.write_packet(&packet).expect("write");
    }
    muxer.finish().expect("finish");
    io.commit("out.flac").expect("commit");
    io.get("out.flac").expect("output")
}

/// A FLAC whose STREAMINFO says 20 significant bits, as a 20-bit master would. `flac` must
/// hold 24-bit samples with the low four bits clear (see [`wav`]). The frames still say 24,
/// so it is for reading and probing, not for decoding.
pub fn labelled_20_bit(flac: &[u8]) -> Vec<u8> {
    let mut bytes = flac.to_vec();
    let mut info = StreamInfoBlock::parse(&bytes[8..42]).expect("streaminfo");
    info.bits_per_sample = 20;
    bytes[8..42].copy_from_slice(&info.write());
    bytes
}

/// One Ogg page holding a Vorbis identification packet's first bytes: enough for the
/// reader to name the codec and refuse it.
pub fn ogg_vorbis() -> Vec<u8> {
    let packet = b"\x01vorbis\0\0\0\0\x02\x44\xac\0\0";
    let mut out = b"OggS".to_vec();
    out.extend([0, 0x03]);
    out.extend(0i64.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend([0; 4]);
    out.push(1);
    out.push(u8::try_from(packet.len()).expect("short"));
    out.extend(packet);
    let mut crc = 0u32;
    for byte in &out {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    out[22..26].copy_from_slice(&crc.to_le_bytes());
    out
}
