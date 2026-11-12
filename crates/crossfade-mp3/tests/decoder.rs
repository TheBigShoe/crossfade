use crossfade_mp3::{Decoder, Error, Layer, OUTPUT_LIMIT};

/// MPEG-1 Layer III, 128 kbit/s, 44.1 kHz: mono without CRC, mono with CRC, joint stereo.
const MONO: [u8; 4] = [0xFF, 0xFB, 0x90, 0xC4];
const MONO_CRC: [u8; 4] = [0xFF, 0xFA, 0x90, 0xC4];
const JOINT: [u8; 4] = [0xFF, 0xFB, 0x90, 0x74];
/// MPEG-2.5 Layer III, 8 kbit/s, 8 kHz, joint stereo with intensity and mid/side.
const LSF_JOINT: [u8; 4] = [0xFF, 0xE3, 0x18, 0x74];

fn frame(h: [u8; 4], len: usize) -> Vec<u8> {
    let mut f = vec![0u8; len];
    f[..4].copy_from_slice(&h);
    f
}

/// The CRC-16 the standard puts after the header: polynomial 0x8005 over header bytes 2-3
/// and the side information.
fn crc(f: &[u8], side_end: usize) -> u16 {
    let mut c: u16 = 0xFFFF;
    for &b in f[2..4].iter().chain(&f[6..side_end]) {
        for i in (0..8).rev() {
            let bit = (b >> i) & 1 != 0;
            let top = c & 0x8000 != 0;
            c <<= 1;
            if bit != top {
                c ^= 0x8005;
            }
        }
    }
    c
}

#[test]
fn a_frame_without_main_data_decodes_to_exact_silence() {
    let mut d = Decoder::new();
    let mut out = vec![9.0];
    let info = d.decode_frame(&frame(JOINT, 417), &mut out).unwrap();
    assert_eq!((info.samples, info.header.channels()), (1152, 2));
    assert_eq!(out.len(), 1 + 2304);
    assert!(out[1..].iter().all(|&s| s == 0.0));
}

#[test]
fn main_data_from_before_the_start_is_a_reservoir_error_with_silence() {
    let mut d = Decoder::new();
    let mut f = frame(MONO, 417);
    // main_data_begin = 300: nothing is held yet.
    f[4] = (300 >> 1) as u8;
    f[5] = ((300 & 1) << 7) as u8;
    let mut out = Vec::new();
    let e = d.decode_frame(&f, &mut out).unwrap_err();
    assert_eq!(e, Error::Reservoir { needed: 300, available: 0 });
    assert!(e.is_reservoir());
    assert_eq!(out, vec![0.0; 1152]);
    // The frame's own 396 bytes of main data are held for the next one.
    let e = d.decode_frame(&f, &mut out).unwrap();
    assert_eq!((e.samples, out.len()), (1152, 2304));
    // A reset forgets them.
    d.reset();
    assert!(d.decode_frame(&f, &mut out).unwrap_err().is_reservoir());
}

#[test]
fn layers_one_and_two_give_silence_and_say_so() {
    let mut d = Decoder::new();
    let mut out = Vec::new();
    let l2 = frame([0xFF, 0xFD, 0x84, 0x00], 384);
    assert_eq!(d.decode_frame(&l2, &mut out), Err(Error::UnsupportedLayer(Layer::II)));
    assert_eq!(out.len(), 2304);
}

#[test]
fn not_a_header_appends_nothing() {
    let mut d = Decoder::new();
    let mut out = Vec::new();
    assert_eq!(d.decode_frame(&[0xFF, 0xFB], &mut out), Err(Error::NotAHeader("shorter than a header")));
    assert_eq!(d.decode_frame(&[0; 417], &mut out), Err(Error::NotAHeader("no frame sync")));
    assert!(out.is_empty());
}

#[test]
fn a_frame_of_the_wrong_length_is_refused_with_silence() {
    let mut d = Decoder::new();
    let mut out = Vec::new();
    let e = d.decode_frame(&frame(MONO, 416), &mut out).unwrap_err();
    assert_eq!(e, Error::FrameLength { expected: 417, actual: 416 });
    assert_eq!(out.len(), 1152);
    // Free format: any length from the end of the side information, up to the cap.
    let free = [0xFF, 0xFB, 0x00, 0xC4];
    assert!(d.decode_frame(&frame(free, 21), &mut out).is_ok());
    assert!(matches!(d.decode_frame(&frame(free, 20), &mut out), Err(Error::FrameLength { .. })));
    assert!(matches!(
        d.decode_frame(&frame(free, crossfade_mp3::MAX_FRAME_LEN + 1), &mut out),
        Err(Error::FrameLength { .. })
    ));
}

#[test]
fn the_crc_is_checked() {
    let mut d = Decoder::new();
    let mut out = Vec::new();
    let mut f = frame(MONO_CRC, 417);
    let c = crc(&f, 6 + 17);
    f[4..6].copy_from_slice(&c.to_be_bytes());
    assert!(d.decode_frame(&f, &mut out).is_ok());
    f[10] ^= 0x40;
    let e = d.decode_frame(&f, &mut out).unwrap_err();
    assert!(matches!(e, Error::Crc { expected, .. } if expected == c), "{e:?}");
    assert_eq!(out.len(), 2 * 1152);
}

#[test]
fn impossible_side_information_is_corrupt() {
    let mut d = Decoder::new();
    let mut out = Vec::new();
    // big_values is 9 bits after main_data_begin (9), private bits (5), scfsi (4) and
    // part2_3_length (12): bits 30-38 of the side information.
    let mut f = frame(MONO, 417);
    f[4 + 3] = 0b0000_0011;
    f[4 + 4] = 0xFF;
    assert_eq!(d.decode_frame(&f, &mut out), Err(Error::Corrupt("big_values above 288")));
    // part2_3_length larger than the frame's main data.
    let mut f = frame(MONO, 417);
    f[4 + 2] = 0b0011_1111;
    f[4 + 3] = 0b1111_1100;
    assert_eq!(
        d.decode_frame(&f, &mut out),
        Err(Error::Corrupt("granules longer than the main data"))
    );
    assert_eq!(out.len(), 2 * 1152);
}

/// A small deterministic generator, so failures reproduce.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn any_bytes_after_a_header_give_finite_bounded_output_without_panicking() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut d = Decoder::new();
    let mut out = Vec::new();
    for (h, len) in [(JOINT, 417), (MONO, 417), (LSF_JOINT, 72), ([0xFF, 0xE3, 0x18, 0x10], 72)] {
        for _ in 0..400 {
            let mut f = frame(h, len);
            for b in &mut f[4..] {
                *b = rng.next() as u8;
            }
            // Mostly small main_data_begin, so the reservoir is used.
            f[4] &= 0x1F;
            out.clear();
            let _ = d.decode_frame(&f, &mut out);
            assert!(
                out.iter().all(|s| s.is_finite() && s.abs() <= OUTPUT_LIMIT),
                "{h:02X?}"
            );
        }
    }
}

#[test]
fn reset_makes_a_decoder_as_good_as_new() {
    let mut rng = Rng(42);
    let frames: Vec<Vec<u8>> = (0..12)
        .map(|_| {
            let mut f = frame(JOINT, 417);
            for b in &mut f[4..] {
                *b = rng.next() as u8;
            }
            f[4] &= 0x0F;
            f
        })
        .collect();
    let run = |d: &mut Decoder| {
        let mut out = Vec::new();
        for f in &frames {
            let _ = d.decode_frame(f, &mut out);
        }
        out
    };
    let fresh = run(&mut Decoder::new());
    let mut used = Decoder::new();
    run(&mut used);
    used.reset();
    assert_eq!(run(&mut used), fresh);
}
