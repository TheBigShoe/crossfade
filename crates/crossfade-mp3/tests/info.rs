use crossfade_mp3::FrameHeader;
use crossfade_mp3::info::{InfoFrame, InfoKind};

fn header(b: [u8; 4]) -> FrameHeader {
    FrameHeader::parse(&b).unwrap()
}

/// LAME 4.0's first frame for one second of 44.1 kHz stereo at 128 kbit/s CBR: `Info`, all four
/// fields, then the LAME extension with delay 576 and padding 1404.
fn lame_cbr() -> Vec<u8> {
    let mut f = vec![0u8; 417];
    f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
    let mut t = b"Info".to_vec();
    t.extend(15u32.to_be_bytes());
    t.extend(40u32.to_be_bytes());
    t.extend(17135u32.to_be_bytes());
    t.extend((0..100u8).map(|i| i * 2));
    t.extend(57u32.to_be_bytes());
    t.extend(b"LAME4.0 \0");
    t.extend([
        0x01, 0xAA, 0, 0, 0, 0, 0x2E, 0x99, 0, 0, 0x14, 0x80, 0x24, 0x05, 0x7C,
    ]);
    f[36..36 + t.len()].copy_from_slice(&t);
    f
}

#[test]
fn lame_info_frame_gives_counts_and_gapless_figures() {
    let f = lame_cbr();
    let info = InfoFrame::parse(&header([0xFF, 0xFB, 0x90, 0x64]), &f).unwrap();
    assert_eq!(info.kind, InfoKind::Info);
    assert_eq!((info.frames, info.bytes), (Some(40), Some(17135)));
    assert_eq!(info.encoder, Some(*b"LAME4.0 \0"));
    assert_eq!(info.delay_padding, Some((576, 1404)));
}

#[test]
fn xing_fields_are_each_optional() {
    // Mono MPEG-2: the tag sits after 4 + 9 bytes; only the frame count is present.
    let h = header([0xFF, 0xF3, 0x50, 0xC4]);
    let mut f = vec![0u8; 130];
    f[..4].copy_from_slice(&[0xFF, 0xF3, 0x50, 0xC4]);
    f[13..17].copy_from_slice(b"Xing");
    f[17..21].copy_from_slice(&1u32.to_be_bytes());
    f[21..25].copy_from_slice(&1234u32.to_be_bytes());
    let info = InfoFrame::parse(&h, &f).unwrap();
    assert_eq!(
        (info.kind, info.frames, info.bytes),
        (InfoKind::Xing, Some(1234), None)
    );
    assert_eq!((info.encoder, info.delay_padding), (None, None));
    // With a CRC the side information starts two bytes later.
    let h = header([0xFF, 0xF2, 0x50, 0xC4]);
    let mut f = vec![0u8; 130];
    f[15..19].copy_from_slice(b"Xing");
    f[19..23].copy_from_slice(&2u32.to_be_bytes());
    f[23..27].copy_from_slice(&999u32.to_be_bytes());
    let info = InfoFrame::parse(&h, &f).unwrap();
    assert_eq!((info.frames, info.bytes), (None, Some(999)));
}

#[test]
fn vbri_sits_32_bytes_after_the_header() {
    let h = header([0xFF, 0xFB, 0x90, 0x64]);
    let mut f = vec![0u8; 417];
    f[36..40].copy_from_slice(b"VBRI");
    f[46..50].copy_from_slice(&5000u32.to_be_bytes());
    f[50..54].copy_from_slice(&77u32.to_be_bytes());
    let info = InfoFrame::parse(&h, &f).unwrap();
    assert_eq!(
        (info.kind, info.frames, info.bytes),
        (InfoKind::Vbri, Some(77), Some(5000))
    );
}

#[test]
fn audio_frames_and_scraps_are_not_info_frames() {
    let h = header([0xFF, 0xFB, 0x90, 0x64]);
    assert_eq!(InfoFrame::parse(&h, &[0xAB; 417]), None);
    let f = lame_cbr();
    for cut in [0, 4, 36, 38, 40, 44, 60, 170, 180] {
        // Never panics; a tag cut inside a field loses that field.
        let _ = InfoFrame::parse(&h, &f[..cut]);
    }
    assert_eq!(InfoFrame::parse(&h, &f[..39]), None);
    let cut = InfoFrame::parse(&h, &f[..50]).unwrap();
    assert_eq!((cut.frames, cut.bytes), (Some(40), None));
    let layer2 = header([0xFF, 0xFD, 0x84, 0x00]);
    assert_eq!(InfoFrame::parse(&layer2, &f), None);
    // An unknown encoder string after the Xing fields is not LAME's extension.
    let mut other = f.clone();
    other[156..160].copy_from_slice(b"XXXX");
    let info = InfoFrame::parse(&h, &other).unwrap();
    assert_eq!((info.encoder, info.delay_padding), (None, None));
}
