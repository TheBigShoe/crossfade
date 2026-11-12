use super::*;
use crate::open_demuxer;
use crossfade_core::{ChannelLayout, CodecId, Error, Limits, Packet, SampleFormat, TimeBase};
use crossfade_testkit::io::FaultySource;
use std::io::Cursor;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/adts");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}/{name}")).unwrap()
}

/// An MPEG-4 AAC LC header, 44.1 kHz, `channels` configuration, no CRC, one block.
fn header(payload: usize, channels: u8) -> Vec<u8> {
    let len = payload + 7;
    vec![
        0xFF,
        0xF1,
        0x50 | (channels >> 2),
        (channels & 3) << 6 | (len >> 11) as u8,
        (len >> 3) as u8,
        ((len & 7) as u8) << 5 | 0x1F,
        0xFC,
    ]
}

/// A frame whose payload is `fill` repeated, so packets can be told apart.
fn frame(payload: usize, fill: u8) -> Vec<u8> {
    let mut f = header(payload, 2);
    f.extend(std::iter::repeat_n(fill, payload));
    f
}

fn frames(n: usize) -> Vec<u8> {
    (0..n).flat_map(|i| frame(10 + i % 7, i as u8)).collect()
}

fn open_bytes(bytes: Vec<u8>) -> Result<Box<dyn Demuxer>, Error> {
    open(Box::new(Cursor::new(bytes)), &Limits::default())
}

fn open_pipe(bytes: Vec<u8>) -> Result<Box<dyn Demuxer>, Error> {
    let src = FaultySource::new(Cursor::new(bytes)).not_seekable();
    open(Box::new(src), &Limits::default())
}

fn packets(d: &mut dyn Demuxer) -> Vec<Packet> {
    let mut out = Vec::new();
    let mut p = Packet::default();
    while d.read_packet(&mut p).unwrap() {
        out.push(p.clone());
    }
    out
}

/// Payloads by walking `frame_length` from the start: what the reader must hand out.
fn walk(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 7 <= bytes.len() {
        let h = crossfade_aac::adts::Header::parse(&bytes[at..]).unwrap();
        let end = at + usize::from(h.frame_length);
        if end > bytes.len() {
            break;
        }
        out.push(bytes[at + h.header_len()..end].to_vec());
        at = end;
    }
    out
}

#[test]
fn every_fixture_opens_with_its_stream_and_every_frame_as_a_packet() {
    for (name, rate, layout, count) in [
        ("tone_44k_stereo.aac", 44100, ChannelLayout::stereo(), 46),
        ("sweep_48k_mono.aac", 48000, ChannelLayout::mono(), 49),
        ("clicks_44k_stereo.aac", 44100, ChannelLayout::stereo(), 46),
        ("noise_44k_stereo.aac", 44100, ChannelLayout::stereo(), 24),
        ("silence_44k_stereo.aac", 44100, ChannelLayout::stereo(), 24),
        ("tone_16k_mono.aac", 16000, ChannelLayout::mono(), 18),
        ("tone_8k_mono.aac", 8000, ChannelLayout::mono(), 10),
        ("mix_44k_stereo_64k.aac", 44100, ChannelLayout::stereo(), 46),
        // HE-AAC: ADTS describes the core.
        ("mix_44k_stereo_he.aac", 22050, ChannelLayout::stereo(), 25),
        (
            "tones_48k_51.aac",
            48000,
            ChannelLayout::from_name("5.1").unwrap(),
            26,
        ),
    ] {
        let bytes = fixture(name);
        let mut d = open_bytes(bytes.clone()).unwrap();
        let s = d.streams()[0].clone();
        assert_eq!(s.codec, CodecId::Aac, "{name}");
        assert_eq!(s.audio.format(), SampleFormat::F32, "{name}");
        assert_eq!(s.audio.sample_rate(), rate, "{name}");
        assert_eq!(s.audio.layout(), &layout, "{name}");
        assert_eq!(
            s.time_base,
            TimeBase::new(1, i64::from(rate)).unwrap(),
            "{name}"
        );
        assert_eq!(s.frames, Some(count as u64 * 1024), "{name}");
        let h = crossfade_aac::adts::Header::parse(&bytes).unwrap();
        assert_eq!(s.extradata, h.audio_specific_config(), "{name}");
        let got = packets(&mut *d);
        assert_eq!(got.len(), count, "{name}");
        for (i, p) in got.iter().enumerate() {
            assert_eq!((p.pts, p.duration), (i as i64 * 1024, 1024), "{name} {i}");
        }
        let data: Vec<_> = got.into_iter().map(|p| p.data).collect();
        assert_eq!(data, walk(&bytes), "{name}");
    }
}

#[test]
fn the_sniffer_routes_adts_here_even_behind_id3_and_on_a_pipe() {
    let mut bytes = b"ID3\x04\0\0\0\0\0\x05tags!".to_vec();
    bytes.extend(fixture("tone_44k_stereo.aac"));
    let src = FaultySource::new(Cursor::new(bytes))
        .not_seekable()
        .max_read(5);
    let (id, mut d) = open_demuxer(Box::new(src), &Limits::default()).unwrap();
    assert_eq!(id, crate::FormatId::Adts);
    assert_eq!(d.streams()[0].frames, None);
    assert_eq!(packets(&mut *d).len(), 46);
    // Seekable, the tag is skipped all the same and the index counts the frames.
    let mut bytes = b"ID3\x04\0\0\0\0\0\x05tags!".to_vec();
    bytes.extend(fixture("tone_44k_stereo.aac"));
    let (_, mut d) = open_demuxer(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    assert_eq!(d.streams()[0].frames, Some(46 * 1024));
    assert_eq!(d.seek(5000).unwrap(), 3 * 1024);
    assert_eq!(packets(&mut *d).len(), 43);
}

#[test]
fn a_pipe_gives_the_same_packets_with_no_length_and_no_seeking() {
    let bytes = fixture("clicks_44k_stereo.aac");
    let mut file = open_bytes(bytes.clone()).unwrap();
    let mut pipe = open_pipe(bytes).unwrap();
    assert_eq!(pipe.streams()[0].frames, None);
    assert_eq!(
        pipe.seek(0),
        Err(Error::Unsupported {
            what: "adts: the source cannot seek".into()
        })
    );
    assert_eq!(packets(&mut *pipe), packets(&mut *file));
}

#[test]
fn one_sync_word_without_a_second_header_is_not_adts() {
    // What a FLAC frame header can look like to the sniffer, followed by anything.
    let mut flac_like = header(20, 2);
    flac_like.extend([0x55; 40]);
    assert_eq!(
        open_bytes(flac_like.clone()).err(),
        Some(Error::UnrecognisedFormat)
    );
    assert_eq!(open_pipe(flac_like).err(), Some(Error::UnrecognisedFormat));
    // A second header of another stream does not confirm the first.
    let mut other = frame(20, 1);
    other.extend(header(20, 1));
    other.extend([0; 20]);
    assert_eq!(open_bytes(other).err(), Some(Error::UnrecognisedFormat));
    // Too short to hold the first frame.
    assert_eq!(
        open_bytes(header(20, 2)).err(),
        Some(Error::UnrecognisedFormat)
    );
}

#[test]
fn a_stream_of_exactly_one_frame_is_confirmed_by_its_end() {
    let mut d = open_bytes(frame(30, 9)).unwrap();
    assert_eq!(d.streams()[0].frames, Some(1024));
    assert_eq!(packets(&mut *d).len(), 1);
}

#[test]
fn garbage_between_frames_is_skipped_and_counting_goes_on() {
    let clean = frames(12);
    let cut = walk(&clean)[..5].iter().map(|p| p.len() + 7).sum::<usize>();
    // Junk that holds a false sync word, whose header has no successor where it says.
    let mut junk = vec![0x11; 50];
    junk.extend(header(3, 2));
    junk.extend([0x22; 30]);
    let dirty = [&clean[..cut], &junk, &clean[cut..]].concat();
    for d in [open_bytes(dirty.clone()), open_pipe(dirty.clone())] {
        let got = packets(&mut *d.unwrap());
        let want = walk(&clean);
        assert_eq!(got.len(), want.len());
        for (i, (p, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!((p.pts, &p.data), (i as i64 * 1024, w));
        }
    }
    assert_eq!(
        open_bytes(dirty).unwrap().streams()[0].frames,
        Some(12 * 1024)
    );
}

#[test]
fn too_much_garbage_is_malformed() {
    let clean = frames(4);
    let first = walk(&clean)[0].len() + 7;
    let dirty = [
        &clean[..first],
        &vec![0u8; MAX_RESYNC + 1][..],
        &clean[first..],
    ]
    .concat();
    let mut d = open_pipe(dirty.clone()).unwrap();
    let mut p = Packet::default();
    assert!(d.read_packet(&mut p).unwrap());
    assert_eq!(
        d.read_packet(&mut p),
        Err(Error::Malformed {
            what: format!("adts: no frame header in the {MAX_RESYNC} bytes after offset {first}")
        })
    );
    assert!(matches!(
        open_bytes(dirty).err(),
        Some(Error::Malformed { .. })
    ));
}

#[test]
fn a_truncated_last_frame_and_a_trailing_tag_are_dropped() {
    let clean = frames(6);
    let mut cut = clean.clone();
    cut.truncate(clean.len() - 3);
    let mut tagged = clean.clone();
    tagged.extend(b"TAG");
    tagged.extend([0x20; 125]);
    for (bytes, n) in [(cut, 5), (tagged, 6)] {
        assert_eq!(packets(&mut *open_pipe(bytes.clone()).unwrap()).len(), n);
        let mut d = open_bytes(bytes).unwrap();
        assert_eq!(d.streams()[0].frames, Some(n as u64 * 1024));
        assert_eq!(packets(&mut *d).len(), n);
    }
}

#[test]
fn the_crc_is_skipped_with_the_header() {
    let mut bytes = Vec::new();
    for i in 0..3u8 {
        let mut h = header(12, 2);
        h[1] = 0xF0; // protection_absent 0
        let len = 12 + 9;
        h[3] = 2 << 6 | (len >> 11) as u8;
        h[4] = (len >> 3) as u8;
        h[5] = ((len & 7) as u8) << 5 | 0x1F;
        bytes.extend(h);
        bytes.extend([0xC0, 0xC1]);
        bytes.extend([i; 10]);
    }
    let got = packets(&mut *open_bytes(bytes).unwrap());
    assert_eq!(got.len(), 3);
    assert_eq!(got[2].data, [2; 10]);
}

#[test]
fn frames_of_several_raw_data_blocks_are_refused_by_name() {
    let mut bytes = frames(3);
    bytes[6] |= 1; // two blocks in the first frame
    let want = Error::Unsupported {
        what: "adts: frames holding 2 raw data blocks are not supported".into(),
    };
    assert_eq!(open_bytes(bytes.clone()).err(), Some(want.clone()));
    let mut later = frames(3);
    let second = walk(&later)[0].len() + 7;
    later[second + 6] |= 3;
    let mut d = open_pipe(later).unwrap();
    let mut p = Packet::default();
    assert!(d.read_packet(&mut p).unwrap());
    assert_eq!(
        d.read_packet(&mut p),
        Err(Error::Unsupported {
            what: "adts: frames holding 4 raw data blocks are not supported".into()
        })
    );
}

#[test]
fn seeking_lands_one_packet_early_and_reads_on_from_there() {
    let bytes = frames(300);
    let all = packets(&mut *open_bytes(bytes.clone()).unwrap());
    let mut d = open_bytes(bytes).unwrap();
    for (pts, landed) in [
        (0, 0),
        (-5, 0),
        (1023, 0),
        (1024, 0),
        (2048, 1024),
        (5000, 3 * 1024),
        (299 * 1024 + 1, 298 * 1024),
        (150 * 1024, 149 * 1024),
        (10, 0),
    ] {
        assert_eq!(d.seek(pts), Ok(landed), "{pts}");
        let rest = packets(&mut *d);
        assert_eq!(rest, all[(landed / 1024) as usize..], "{pts}");
    }
    for pts in [300 * 1024, 300 * 1024 + 7, i64::MAX] {
        assert_eq!(d.seek(pts), Ok(300 * 1024), "{pts}");
        assert!(!d.read_packet(&mut Packet::default()).unwrap());
    }
}

#[test]
fn the_seek_index_stays_coarse_for_long_streams() {
    // Empty frames are the densest stream there is.
    let n = 200_000;
    let bytes: Vec<u8> = (0..n).flat_map(|_| header(0, 1)).collect();
    let mut reader = AdtsReader::open(Box::new(Cursor::new(bytes)), &Limits::default()).unwrap();
    assert_eq!(reader.streams()[0].frames, Some(n as u64 * 1024));
    assert!(reader.index_len() <= MAX_INDEX, "{}", reader.index_len());
    assert_eq!(reader.seek(123_457 * 1024), Ok(123_456 * 1024));
    let mut p = Packet::default();
    assert!(reader.read_packet(&mut p).unwrap());
    assert_eq!(p.pts, 123_456 * 1024);
}

#[test]
fn channel_configuration_zero_takes_its_channels_from_the_first_blocks_pce() {
    // A PCE (id 5) for one front pair and one LFE, then END, as the raw data block.
    // id 101, tag 0000, profile 01, index 0100, front 0001, side 0000, back 0000, lfe 01,
    // assoc 000, cc 0000, three mixdown flags 000, front CPE 1 0000, lfe 0000, align,
    // comment length 0, END 111.
    let pce = [0xA0, 0xA0, 0x80, 0x20, 0x04, 0x00, 0x00, 0xE0];
    let mut bytes = Vec::new();
    for _ in 0..3 {
        bytes.extend(header(pce.len(), 0));
        bytes.extend(pce);
    }
    let pc = crossfade_aac::asc::ProgramConfig::from_raw_data_block(&pce)
        .unwrap()
        .unwrap();
    assert_eq!(pc.channels(), 3, "{pc:?}");
    let d = open_bytes(bytes).unwrap();
    assert_eq!(
        d.streams()[0].audio.layout(),
        &ChannelLayout::unpositioned(3).unwrap()
    );
    // Without one there is no telling.
    let mut none = Vec::new();
    for _ in 0..3 {
        none.extend(header(2, 0));
        none.extend([0xE0, 0]);
    }
    assert_eq!(
        open_bytes(none).err(),
        Some(Error::Unsupported {
            what: "adts: channel configuration 0 without a program config element first".into()
        })
    );
}

#[test]
fn a_source_error_is_reported() {
    let bytes = frames(20);
    let src = FaultySource::new(Cursor::new(bytes)).fail_at(100, std::io::ErrorKind::Other);
    assert!(matches!(
        open(Box::new(src), &Limits::default()).err(),
        Some(Error::Io { .. })
    ));
}
