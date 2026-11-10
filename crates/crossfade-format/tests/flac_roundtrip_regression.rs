//! Inputs the `flac_roundtrip` fuzz target found: engine encoder, FLAC muxer, FLAC demuxer and
//! decoder must give back exactly the samples that went in.
use crossfade_codec::{EncodeOptions, decoder_for, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Limits, MediaSink, MediaSource, Packet,
    SampleFormat, Samples, StreamInfo, Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer};
use std::io::{self, Cursor, SeekFrom};
use std::sync::{Arc, Mutex};

struct Pipe(Vec<u8>, usize);

impl MediaSource for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let rest = &self.0[self.1..];
        let n = buf.len().min(rest.len());
        buf[..n].copy_from_slice(&rest[..n]);
        self.1 += n;
        Ok(n)
    }
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "pipe"))
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
    fn is_seekable(&self) -> bool {
        false
    }
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Cursor<Vec<u8>>>>);

impl MediaSink for Shared {
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        MediaSink::write_all(&mut *self.0.lock().unwrap(), buf)
    }
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        MediaSink::seek(&mut *self.0.lock().unwrap(), pos)
    }
    fn finish(&mut self) -> io::Result<()> {
        MediaSink::finish(&mut *self.0.lock().unwrap())
    }
}

/// The fuzz target's body for 16-bit samples: returns what came back.
fn round_trip(rate: u32, channels: u16, level: u8, samples: Vec<i16>) -> Vec<i16> {
    let spec = AudioSpec::new(
        rate,
        SampleFormat::S16,
        ChannelLayout::unpositioned(channels).unwrap(),
    )
    .unwrap();
    let mut enc = encoder_for(
        CodecId::Flac,
        &spec,
        &EncodeOptions {
            compression_level: Some(level),
        },
    )
    .unwrap();
    let mut info = StreamInfo::new(
        0,
        CodecId::Flac,
        spec.clone(),
        TimeBase::new(1, i64::from(rate)).unwrap(),
    );
    info.extradata = enc.extradata().to_vec();
    let sink = Shared::default();
    let mut mux =
        create_muxer(FormatId::Flac, Box::new(sink.clone()), &info, &Tags::new()).unwrap();
    let mut packet = Packet::default();
    let mut frame = AudioFrame::new(&spec);
    frame.samples = Samples::S16(samples);
    enc.send_frame(&frame).unwrap();
    enc.drain();
    while enc.receive_packet(&mut packet).unwrap() {
        mux.write_packet(&packet).unwrap();
    }
    mux.set_extradata(enc.extradata()).unwrap();
    mux.finish().unwrap();
    let bytes = sink.0.lock().unwrap().get_ref().clone();

    let mut demux = crossfade_format::flac::open(Box::new(Pipe(bytes, 0)), &Limits::default())
        .expect("our own output opens");
    let stream = demux.streams()[0].clone();
    let mut dec = decoder_for(&stream).unwrap();
    let mut out = AudioFrame::new(&stream.audio);
    let mut back = Vec::new();
    while demux.read_packet(&mut packet).unwrap() {
        dec.send_packet(&packet).unwrap();
        while dec.receive_frame(&mut out).unwrap() {
            let Samples::S16(v) = &out.samples else {
                panic!()
            };
            back.extend_from_slice(v);
        }
    }
    dec.drain();
    while dec.receive_frame(&mut out).unwrap() {
        let Samples::S16(v) = &out.samples else {
            panic!()
        };
        back.extend_from_slice(v);
    }
    back
}

#[test]
fn the_first_fuzz_crash_round_trips() {
    // 6 header bytes (format, channels, rate, level, ...) then raw little-endian samples.
    let data = include_bytes!("fixtures/flac_roundtrip_crash_1.bin");
    let [_, _, _, level, body @ ..] = &data[..] else {
        unreachable!()
    };
    let channels = u16::from(data[1] % 8) + 1;
    let rate = [
        8000, 11025, 44100, 48000, 96000, 192_000, 384_000, 1_048_575,
    ][usize::from(data[2]) % 8];
    let per = 2 * usize::from(channels);
    let frames = body.len() / per;
    let samples: Vec<i16> = body[..frames * per]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    assert_eq!(data[0] % 4, 1, "the crash was a 16-bit input");
    assert_eq!(
        round_trip(rate, channels, level % 9, samples.clone()),
        samples
    );
}
