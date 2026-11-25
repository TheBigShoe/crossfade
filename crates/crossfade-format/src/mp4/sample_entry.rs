//! The sample description a codec needs in an MP4 track, the writer's side of `entry.rs`. A
//! codec is added by giving `entry` another arm.

use super::build::{boxed, full};
use crossfade_core::{CodecId, Error, StreamInfo};

pub(super) fn refused(codec: CodecId) -> Error {
    Error::Unsupported {
        what: format!("writing {} into mp4 is not supported yet", codec.name()),
    }
}

/// The track's `stsd` entry for `stream`; `cookie` is the codec's header as `extradata`
/// carries it.
pub(super) fn entry(stream: &StreamInfo, cookie: &[u8]) -> Result<Vec<u8>, Error> {
    match stream.codec {
        CodecId::Alac => alac(stream, cookie),
        other => Err(refused(other)),
    }
}

/// An audio sample entry (version 0) around `children`.
fn sound(
    kind: &[u8; 4],
    channels: u16,
    bits: u16,
    rate: u32,
    children: &[u8],
) -> Result<Vec<u8>, Error> {
    let mut head = vec![0u8; 6];
    head.extend_from_slice(&1u16.to_be_bytes()); // data reference index
    head.extend_from_slice(&[0; 8]);
    head.extend_from_slice(&channels.to_be_bytes());
    head.extend_from_slice(&bits.to_be_bytes());
    head.extend_from_slice(&[0; 4]);
    // 16.16 fixed point: a rate past 65535 does not fit, and the codec's header has the real
    // one.
    let fixed = u16::try_from(rate).map_or(0, |r| u32::from(r) << 16);
    head.extend_from_slice(&fixed.to_be_bytes());
    boxed(kind, &[&head, children])
}

/// Core Audio's layout tag for ALAC's layout at each channel count 3 to 8 (the values
/// `afconvert` writes: layout number in the high half, channels in the low).
fn alac_layout_tag(channels: u8) -> Option<u32> {
    let layout = match channels {
        3 => 114,
        4 => 116,
        5 => 120,
        6 => 124,
        7 => 142,
        8 => 127,
        _ => return None,
    };
    Some(layout << 16 | u32::from(channels))
}

fn alac(stream: &StreamInfo, cookie: &[u8]) -> Result<Vec<u8>, Error> {
    let config = crossfade_alac::Config::parse(cookie).map_err(|e| Error::Unsupported {
        what: format!("mp4: {e}"),
    })?;
    let inner = full(b"alac", 0, 0, &[&cookie[..crossfade_alac::Config::LEN]])?;
    // Mono and stereo are what a reader assumes; more channels need their layout said.
    let chan = match alac_layout_tag(config.channels) {
        Some(tag) => full(b"chan", 0, 0, &[&tag.to_be_bytes(), &[0; 8]])?,
        None => Vec::new(),
    };
    sound(
        b"alac",
        u16::from(config.channels),
        u16::from(config.bit_depth),
        stream.audio.sample_rate(),
        &[inner, chan].concat(),
    )
}
