//! Inputs the `flac_roundtrip` fuzz target found.
use crossfade_codec::{EncodeOptions, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Packet, SampleFormat, Samples,
};

/// A frame's coded audio can hold a bit pattern that is a valid frame header right after a
/// prefix whose CRC-16 is zero (the fuzz target found one at about 2^-16 per false header). A
/// demuxer that splits by scanning for syncs and checking the CRC cannot tell that from a frame
/// boundary; libFLAC avoids it only by decoding as it goes. The `flac_roundtrip` target skips
/// such inputs by the same test as this one.
pub fn ambiguous(frame: &[u8]) -> bool {
    use crossfade_flac::crc::crc16;
    use crossfade_flac::frame::parse_header;
    let Ok(h) = parse_header(frame) else {
        return false;
    };
    (h.header_len + 2..frame.len().saturating_sub(1))
        .any(|i| crc16(&frame[..i]) == 0 && parse_header(&frame[i..]).is_ok())
}

#[test]
fn the_first_fuzz_crash_is_an_ambiguous_frame_the_demuxer_splits_early() {
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
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c))
        .collect();
    assert_eq!(data[0] % 4, 1, "the crash was a 16-bit input");
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
            compression_level: Some(level % 9),
        },
    )
    .unwrap();
    let mut frame = AudioFrame::new(&spec);
    frame.samples = Samples::S16(samples);
    enc.send_frame(&frame).unwrap();
    enc.drain();
    let mut packet = Packet::default();
    assert!(enc.receive_packet(&mut packet).unwrap());
    assert!(ambiguous(&packet.data));
}
