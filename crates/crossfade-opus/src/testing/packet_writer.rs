//! Writes Opus packets (RFC 6716 section 3 and Appendix B) from their parts, every field a
//! raw choice so that a test can build valid packets and, by changing one byte, invalid ones.

/// How the frames of a packet are packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packing {
    /// Code 0: one frame.
    One,
    /// Code 1: two frames of equal length.
    TwoEqual,
    /// Code 2: two frames, the first's length coded.
    TwoDifferent,
    /// Code 3: `frames.len()` frames (1 to 48), CBR (all equal) or VBR; `padding`, when set,
    /// raises the padding flag and appends that many bytes of Opus padding (its length bytes
    /// counted apart).
    Signalled { vbr: bool, padding: Option<usize> },
}

/// The one- or two-byte length of RFC 6716 3.2.1 (`len` at most 1275).
pub fn length_bytes(len: usize) -> Vec<u8> {
    assert!(len <= 1275);
    if len < 252 {
        vec![len as u8]
    } else {
        let first = 252 + (len & 3);
        vec![first as u8, ((len - first) / 4) as u8]
    }
}

/// A packet with TOC `config`/`stereo`, the frames packed as `packing` says; `self_delimited`
/// adds Appendix B's extra length. Panics if the frames do not suit the packing (equal lengths
/// for codes 1 and CBR 3, one frame for code 0, two for codes 1 and 2).
pub fn write_packet(
    config: u8,
    stereo: bool,
    frames: &[&[u8]],
    packing: Packing,
    self_delimited: bool,
) -> Vec<u8> {
    let code = match packing {
        Packing::One => 0,
        Packing::TwoEqual => 1,
        Packing::TwoDifferent => 2,
        Packing::Signalled { .. } => 3,
    };
    let mut out = vec![(config << 3) | (u8::from(stereo) << 2) | code];
    let last = frames.last().expect("at least one frame").len();
    match packing {
        Packing::One => assert_eq!(frames.len(), 1),
        Packing::TwoEqual => {
            assert_eq!(frames.len(), 2);
            assert_eq!(frames[0].len(), frames[1].len());
        }
        Packing::TwoDifferent => {
            assert_eq!(frames.len(), 2);
            out.extend(length_bytes(frames[0].len()));
        }
        Packing::Signalled { vbr, padding } => {
            assert!((1..=48).contains(&frames.len()));
            let p = u8::from(padding.is_some());
            out.push((u8::from(vbr) << 7) | (p << 6) | frames.len() as u8);
            if let Some(padding) = padding {
                // 255 adds 254 and asks for another byte; the last byte is 0..=254.
                let mut rest = padding;
                while rest > 254 {
                    out.push(255);
                    rest -= 254;
                }
                out.push(rest as u8);
            }
            if vbr {
                for f in &frames[..frames.len() - 1] {
                    out.extend(length_bytes(f.len()));
                }
            } else {
                assert!(frames.iter().all(|f| f.len() == last));
            }
        }
    }
    if self_delimited {
        out.extend(length_bytes(last));
    }
    for f in frames {
        out.extend_from_slice(f);
    }
    if let Packing::Signalled {
        padding: Some(padding),
        ..
    } = packing
    {
        out.extend(std::iter::repeat_n(0, padding));
    }
    out
}
