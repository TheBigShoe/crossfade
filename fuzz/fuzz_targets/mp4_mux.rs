#![no_main]
//! The MP4 writer's round trip: an ALAC stream of any channel count, depth and rate, with
//! packets of any bytes and a last packet of any length, reads back through our reader with
//! the same stream, tags and packets. Panics by design when that fails.

use arbitrary::{Result, Unstructured};
use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Packet, SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_format::{FormatId, create_muxer};
use crossfade_fuzz::{SharedSink, limits};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fn run(data: &[u8]) -> Result<()> {
    let mut u = Unstructured::new(data);
    let channels = u.int_in_range(1..=8u8)?;
    let (depth, format) = *u.choose(&[
        (16u8, SampleFormat::S16),
        (20, SampleFormat::S24),
        (24, SampleFormat::S24),
        (32, SampleFormat::S32),
    ])?;
    let rate = u.int_in_range(1..=384_000u32)?;
    let last = u.int_in_range(1..=4096u64)?;
    let titled = u.arbitrary::<bool>()?;
    let mut packets = vec![];
    while !u.is_empty() && packets.len() < 200 {
        let len = u.int_in_range(1..=300usize)?;
        let mut bytes = vec![0; len];
        // Out of input: the rest of the packet stays zero.
        let _ = u.fill_buffer(&mut bytes);
        packets.push(Packet {
            stream: 0,
            pts: packets.len() as i64 * 4096,
            duration: 4096,
            data: bytes,
        });
    }
    if let Some(p) = packets.last_mut() {
        p.duration = last;
    }

    let wave = crossfade_alac::wave_layout(channels).expect("1 to 8 channels");
    let layout = ChannelLayout::from_mask(wave.mask, u16::from(channels)).expect("ALAC's layout");
    let audio = AudioSpec::new(rate, format, layout).expect("a valid spec");
    let base = TimeBase::new(1, i64::from(rate)).expect("a positive rate");
    let mut stream = StreamInfo::new(0, CodecId::Alac, audio, base);
    stream.extradata = crossfade_alac::Config::new(rate, channels, depth)
        .expect("a stream ALAC writes")
        .write()
        .to_vec();
    stream.frames = Some(packets.iter().map(|p| p.duration).sum());
    stream.bits_per_raw_sample = (depth == 20).then_some(20);
    let mut tags = Tags::new();
    if titled {
        tags.push("TITLE", "fuzz");
    }

    let sink = SharedSink::default();
    let mut m = create_muxer(FormatId::Mp4, Box::new(sink.clone()), &stream, &tags)
        .expect("an ALAC stream can be written");
    for p in &packets {
        m.write_packet(p).expect("a memory sink takes every packet");
    }
    m.finish().expect("a memory sink finishes");

    let mut copy = crossfade_format::mp4::open(Box::new(Cursor::new(sink.bytes())), &limits())
        .expect("the file opens");
    assert_eq!(copy.streams()[0], stream);
    assert_eq!(copy.tags(), &tags);
    let mut p = Packet::default();
    let mut got = vec![];
    while copy.read_packet(&mut p).expect("the file reads to its end") {
        got.push(p.clone());
    }
    assert!(got == packets, "packets");
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = run(data);
});
