//! The reader and the decoder together, on files made by the reference encoder from signals
//! this repository can regenerate (see `tests/fixtures/flac/README.md`).

#[path = "support/signals.rs"]
mod signals;

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Error, Limits, Packet, Samples, StreamInfo};
use crossfade_flac::frame::{self, ChannelAssignment};
use crossfade_testkit::io::FaultySource;
use proptest::prelude::*;
use std::io::Cursor;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/flac/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

struct Decoded {
    stream: StreamInfo,
    /// Raw sample values, interleaved, with the left-justification undone.
    samples: Vec<i64>,
    modes: Vec<ChannelAssignment>,
}

fn unjustify(frame: &AudioFrame, bits: u32, out: &mut Vec<i64>) {
    match &frame.samples {
        Samples::U8(v) => out.extend(v.iter().map(|&s| (i64::from(s) - 128) >> (8 - bits))),
        Samples::S16(v) => out.extend(v.iter().map(|&s| i64::from(s) >> (16 - bits))),
        Samples::S24(v) => out.extend(v.iter().map(|&s| i64::from(s) >> (24 - bits))),
        Samples::S32(v) => out.extend(v.iter().map(|&s| i64::from(s) >> (32 - bits))),
        other => panic!("unexpected {other:?}"),
    }
}

/// Opens, demuxes and decodes everything; the first error of any stage is the result.
fn decode_bytes(bytes: Vec<u8>) -> Result<Decoded, Error> {
    let source = FaultySource::new(Cursor::new(bytes)).not_seekable();
    let mut demux = crossfade_format::flac::open(Box::new(source), &Limits::default())?;
    let stream = demux.streams()[0].clone();
    let bits = u32::from(
        stream
            .bits_per_raw_sample
            .unwrap_or(stream.audio.format().bits() as u8),
    );
    let mut decoder = decoder_for(&stream)?;
    let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
    let (mut samples, mut modes) = (Vec::new(), Vec::new());
    let mut next_pts = 0i64;
    while demux.read_packet(&mut packet)? {
        // A damaged stream may skip frames; that is the caller's to notice, as here.
        if packet.pts != next_pts {
            return Err(Error::Malformed {
                what: format!("gap before sample {}", packet.pts),
            });
        }
        next_pts += packet.duration as i64;
        modes.push(frame::parse_header(&packet.data).unwrap().channels);
        decoder.send_packet(&packet)?;
        while decoder.receive_frame(&mut frame)? {
            assert_eq!(frame.pts, packet.pts);
            unjustify(&frame, bits, &mut samples);
        }
    }
    decoder.drain();
    while decoder.receive_frame(&mut frame)? {}
    Ok(Decoded {
        stream,
        samples,
        modes,
    })
}

#[test]
fn every_fixture_decodes_bit_exactly_to_its_signal_with_the_md5_checked() {
    for &(file, name) in signals::FIXTURES {
        let want = signals::signal(name);
        let got = decode_bytes(fixture(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        let audio = &got.stream.audio;
        assert_eq!(audio.sample_rate(), want.rate, "{file}");
        assert_eq!(
            usize::from(audio.layout().channels()),
            want.channels,
            "{file}"
        );
        assert_eq!(got.samples.len(), want.samples.len(), "{file}");
        let want_samples: Vec<i64> = want.samples.iter().map(|&s| i64::from(s)).collect();
        assert!(got.samples == want_samples, "{file}: samples differ");
        // Unknown totals and a zeroed hash are the one fixture where nothing is checked.
        let md5_set = got.stream.extradata[18..34].iter().any(|&b| b != 0);
        assert_eq!(md5_set, file != "unknown-total.flac", "{file}");
        assert_eq!(
            got.stream.frames,
            (file != "unknown-total.flac").then_some(want.frames() as u64),
            "{file}"
        );
    }
}

#[test]
fn the_fixtures_between_them_use_every_stereo_mode_and_the_channel_counts() {
    let mut seen = [false; 4];
    for &(file, _) in signals::FIXTURES {
        for m in decode_bytes(fixture(file)).unwrap().modes {
            seen[match m {
                ChannelAssignment::Independent(_) => 0,
                ChannelAssignment::LeftSide => 1,
                ChannelAssignment::SideRight => 2,
                ChannelAssignment::MidSide => 3,
            }] = true;
        }
    }
    assert_eq!(
        seen, [true; 4],
        "independent, left/side, right/side, mid/side"
    );
    let modes = |file: &str| decode_bytes(fixture(file)).unwrap().modes;
    assert!(modes("stereo-modes-m.flac").contains(&ChannelAssignment::LeftSide));
    assert!(modes("stereo-modes-m.flac").contains(&ChannelAssignment::SideRight));
    assert!(modes("stereo-modes-m.flac").contains(&ChannelAssignment::MidSide));
    assert!(
        modes("stereo-modes-none.flac")
            .iter()
            .all(|m| matches!(m, ChannelAssignment::Independent(2)))
    );
    assert!(
        modes("surround-5.1.flac")
            .iter()
            .all(|m| matches!(m, ChannelAssignment::Independent(6)))
    );
}

#[test]
fn the_sniffing_entry_point_reads_a_fixture_too() {
    let source = Cursor::new(fixture("tagged.flac"));
    let (id, mut demux) =
        crossfade_format::open_demuxer(Box::new(source), &Limits::default()).unwrap();
    assert_eq!(id, crossfade_format::FormatId::Flac);
    let mut packet = Packet::default();
    assert!(demux.read_packet(&mut packet).unwrap());
}

/// Offset of the first frame: past any ID3v2 tag, the marker and every metadata block.
fn audio_start(bytes: &[u8]) -> usize {
    let mut at = 0;
    if bytes.starts_with(b"ID3") {
        let size = bytes[6..10]
            .iter()
            .fold(0usize, |a, &b| a << 7 | usize::from(b));
        at = 10 + size;
    }
    assert_eq!(&bytes[at..at + 4], b"fLaC");
    at += 4;
    loop {
        let last = bytes[at] & 0x80 != 0;
        let len = usize::from(bytes[at + 1]) << 16
            | usize::from(bytes[at + 2]) << 8
            | usize::from(bytes[at + 3]);
        at += 4 + len;
        if last {
            return at;
        }
    }
}

#[test]
fn a_changed_byte_in_the_audio_is_caught() {
    for &(file, _) in signals::FIXTURES {
        let good = fixture(file);
        let start = audio_start(&good);
        for at in [start + 10, (start + good.len()) / 2, good.len() - 20] {
            let mut bad = good.clone();
            bad[at] ^= 0x04;
            assert!(decode_bytes(bad).is_err(), "{file}: flip at {at}");
        }
    }
}

#[test]
fn a_corrupt_sample_that_keeps_the_frame_crc_is_caught_by_the_md5() {
    // Re-seal the frame's CRC-16 after changing a bit of verbatim audio: the frame is now
    // well-formed but wrong, which only the hash notices.
    let mut bad = fixture("verbatim-noise.flac");
    let first = bad.windows(2).position(|w| w == [0xFF, 0xF8]).unwrap();
    let mut demux =
        crossfade_format::flac::open(Box::new(Cursor::new(bad.clone())), &Limits::default())
            .unwrap();
    let mut p = Packet::default();
    assert!(demux.read_packet(&mut p).unwrap());
    let len = p.data.len();
    bad[first + len / 2] ^= 0x01;
    let crc = crossfade_flac::crc::crc16(&bad[first..first + len - 2]);
    bad[first + len - 2..first + len].copy_from_slice(&crc.to_be_bytes());
    match decode_bytes(bad) {
        Err(Error::Malformed { what }) => assert!(what.contains("MD5"), "{what}"),
        other => panic!(
            "expected the MD5 error, got {:?}",
            other.map(|d| d.samples.len())
        ),
    }
}

#[test]
fn a_one_byte_per_read_pipe_decodes_the_same() {
    let bytes = fixture("stereo-20bit.flac");
    let want = decode_bytes(bytes.clone()).unwrap().samples;
    let src = FaultySource::new(Cursor::new(bytes))
        .not_seekable()
        .max_read(1);
    let mut demux = crossfade_format::flac::open(Box::new(src), &Limits::default()).unwrap();
    let mut packet = Packet::default();
    let mut n = 0;
    while demux.read_packet(&mut packet).unwrap() {
        n += packet.duration as usize * 2;
    }
    assert_eq!(n, want.len());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn random_byte_flips_never_panic(
        file in 0..signals::FIXTURES.len(),
        flips in proptest::collection::vec((any::<prop::sample::Index>(), 1u8..=255), 1..8),
    ) {
        let mut bytes = fixture(signals::FIXTURES[file].0);
        for (at, mask) in flips {
            let i = at.index(bytes.len());
            bytes[i] ^= mask;
        }
        let _ = decode_bytes(bytes);
    }

    #[test]
    fn truncations_never_panic_and_yield_a_prefix_or_an_error(
        file in 0..signals::FIXTURES.len(),
        cut in any::<prop::sample::Index>(),
    ) {
        let (name, signal) = signals::FIXTURES[file];
        let bytes = fixture(name);
        let n = cut.index(bytes.len());
        if let Ok(d) = decode_bytes(bytes[..n].to_vec()) {
            // Cut between frames: what came out is the start of the signal.
            let want = signals::signal(signal).samples;
            prop_assert!(d.samples.iter().zip(&want).all(|(a, &b)| *a == i64::from(b)));
        }
    }
}
