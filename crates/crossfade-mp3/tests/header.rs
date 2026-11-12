use crossfade_mp3::{ChannelMode, Error, FrameHeader, Layer, Version};

fn parse(b: [u8; 4]) -> Result<FrameHeader, Error> {
    FrameHeader::parse(&b)
}

#[test]
fn mpeg1_layer3_128k_44100_joint_stereo() {
    let h = parse([0xFF, 0xFB, 0x90, 0x64]).unwrap();
    assert_eq!((h.version, h.layer, h.crc), (Version::Mpeg1, Layer::III, false));
    assert_eq!((h.bitrate(), h.sample_rate, h.padding), (128_000, 44100, false));
    assert_eq!((h.mode, h.mode_extension), (ChannelMode::JointStereo, 2));
    assert!(h.ms_stereo() && !h.intensity_stereo());
    assert_eq!(h.channels(), 2);
    assert_eq!(h.frame_len(), Some(417));
    assert_eq!(h.samples_per_frame(), 1152);
    assert_eq!((h.side_info_len(), h.data_start()), (32, 4));
    assert!(h.original && !h.copyright && !h.private);
    let padded = parse([0xFF, 0xFB, 0x92, 0x64]).unwrap();
    assert_eq!(padded.frame_len(), Some(418));
    assert!(padded.private == false && padded.padding);
}

#[test]
fn every_version_and_rate_with_lengths() {
    // (header, version, rate, bitrate, channels, frame length, samples, side info)
    for (b, v, rate, br, ch, len, spf, si) in [
        ([0xFF, 0xFA, 0xE4, 0xC0], Version::Mpeg1, 48000, 320_000, 1, 960, 1152, 17),
        ([0xFF, 0xFB, 0x18, 0x00], Version::Mpeg1, 32000, 32_000, 2, 144, 1152, 32),
        ([0xFF, 0xF3, 0x50, 0xC0], Version::Mpeg2, 22050, 40_000, 1, 130, 576, 9),
        ([0xFF, 0xF3, 0x84, 0x00], Version::Mpeg2, 24000, 64_000, 2, 192, 576, 17),
        ([0xFF, 0xE3, 0x18, 0xC0], Version::Mpeg25, 8000, 8_000, 1, 72, 576, 9),
        ([0xFF, 0xE3, 0x10, 0x40], Version::Mpeg25, 11025, 8_000, 2, 52, 576, 17),
    ] {
        let h = parse(b).unwrap();
        assert_eq!(
            (h.version, h.sample_rate, h.bitrate(), h.channels()),
            (v, rate, br, ch),
            "{b:02X?}"
        );
        assert_eq!(h.frame_len(), Some(len), "{b:02X?}");
        assert_eq!((h.samples_per_frame(), h.side_info_len()), (spf, si), "{b:02X?}");
        assert_eq!(h.data_start(), if b[1] & 1 == 0 { 6 } else { 4 });
    }
}

#[test]
fn layers_one_and_two_parse_for_naming() {
    let l2 = parse([0xFF, 0xFD, 0x84, 0x00]).unwrap();
    assert_eq!((l2.layer, l2.bitrate(), l2.sample_rate), (Layer::II, 128_000, 48000));
    assert_eq!((l2.frame_len(), l2.samples_per_frame(), l2.side_info_len()), (Some(384), 1152, 0));
    let l1 = parse([0xFF, 0xFF, 0x92, 0x00]).unwrap();
    assert_eq!((l1.layer, l1.bitrate(), l1.sample_rate), (Layer::I, 288_000, 44100));
    // 12 * 288000 / 44100 = 78 slots, plus the padding slot, of 4 bytes each.
    assert_eq!((l1.frame_len(), l1.samples_per_frame()), (Some(316), 384));
    let lsf_l2 = parse([0xFF, 0xF5, 0x84, 0x00]).unwrap();
    assert_eq!((lsf_l2.frame_len(), lsf_l2.samples_per_frame()), (Some(384), 1152));
    assert_eq!(Layer::I.name(), "I");
    assert_eq!(Version::Mpeg25.name(), "MPEG-2.5");
}

#[test]
fn free_format_has_no_length_in_the_header() {
    let h = parse([0xFF, 0xFB, 0x02, 0xC0]).unwrap();
    assert!(h.is_free_format());
    assert_eq!((h.bitrate(), h.frame_len()), (0, None));
    assert_eq!(h.free_format_len(1000), 1001);
    let unpadded = parse([0xFF, 0xFB, 0x00, 0xC0]).unwrap();
    assert_eq!(unpadded.free_format_len(1000), 1000);
}

#[test]
fn reserved_fields_and_broken_sync_are_refused() {
    for (b, why) in [
        ([0xFE, 0xFB, 0x90, 0x00], "no frame sync"),
        ([0xFF, 0xDB, 0x90, 0x00], "no frame sync"),
        ([0xFF, 0xEB, 0x90, 0x00], "reserved version"),
        ([0xFF, 0xF9, 0x90, 0x00], "reserved layer"),
        ([0xFF, 0xFB, 0xF0, 0x00], "reserved bitrate"),
        ([0xFF, 0xFB, 0x9C, 0x00], "reserved sample rate"),
    ] {
        assert_eq!(parse(b), Err(Error::NotAHeader(why)), "{b:02X?}");
    }
}

#[test]
fn same_stream_ignores_mode_bitrate_and_flags() {
    let a = parse([0xFF, 0xFB, 0x90, 0x64]).unwrap();
    for b in [[0xFF, 0xFA, 0xE2, 0xC0], [0xFF, 0xFB, 0x10, 0x00]] {
        assert!(a.same_stream(&parse(b).unwrap()), "{b:02X?}");
    }
    for b in [
        [0xFF, 0xFB, 0x94, 0x64], // 48 kHz
        [0xFF, 0xF3, 0x90, 0x64], // MPEG-2
        [0xFF, 0xFD, 0x90, 0x64], // layer II
        [0xFF, 0xFB, 0x00, 0x64], // free format
    ] {
        assert!(!a.same_stream(&parse(b).unwrap()), "{b:02X?}");
    }
}

#[test]
fn modes_and_extensions() {
    let dual = parse([0xFF, 0xFB, 0x90, 0x80]).unwrap();
    assert_eq!((dual.mode, dual.channels()), (ChannelMode::DualChannel, 2));
    let is = parse([0xFF, 0xFB, 0x90, 0x50]).unwrap();
    assert!(is.intensity_stereo() && !is.ms_stereo());
    // The extension means nothing outside joint stereo.
    let plain = parse([0xFF, 0xFB, 0x90, 0x30]).unwrap();
    assert_eq!(plain.mode, ChannelMode::Stereo);
    assert!(!plain.intensity_stereo() && !plain.ms_stereo());
    assert_eq!(parse([0xFF, 0xFB, 0x90, 0x0B]).unwrap().emphasis, 3);
}
