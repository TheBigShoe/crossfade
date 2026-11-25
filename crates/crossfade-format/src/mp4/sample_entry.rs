//! The sample description a codec needs in an MP4 track, the writer's side of `entry.rs`. A
//! codec is added by giving `supports` and `entry` another arm.

use super::build::{boxed, full};
use crossfade_alac::Config;
use crossfade_core::{ChannelLayout, CodecId, Error, SampleFormat, StreamInfo};

pub(super) fn refused(codec: CodecId) -> Error {
    Error::Unsupported {
        what: format!("writing {} into mp4 is not supported yet", codec.name()),
    }
}

/// Whether a track of `codec` can be written.
pub(super) fn supports(codec: CodecId) -> Result<(), Error> {
    match codec {
        CodecId::Alac => Ok(()),
        other => Err(refused(other)),
    }
}

/// The track's `stsd` entry for `stream`, from the codec's header as `extradata` carries it;
/// a header that does not describe the stream is an error, since a reader trusts the header.
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

fn mismatch(what: &str) -> Error {
    Error::Unsupported {
        what: format!("mp4: the alac cookie and the stream disagree: {what}"),
    }
}

/// A cookie is the 24-byte config, then optionally the 24-byte `chan` atom (its size, `chan`,
/// version and flags, the layout tag, two reserved words).
const COOKIE_WITH_CHAN: usize = Config::LEN + 24;

fn alac(stream: &StreamInfo, cookie: &[u8]) -> Result<Vec<u8>, Error> {
    let chan_atom = match cookie.len() {
        Config::LEN => None,
        COOKIE_WITH_CHAN if cookie[Config::LEN + 4..Config::LEN + 8] == *b"chan" => {
            Some(&cookie[Config::LEN..])
        }
        n => {
            return Err(Error::Unsupported {
                what: format!(
                    "mp4: an alac cookie is {} bytes, or {COOKIE_WITH_CHAN} with a chan atom, \
                     not {n}",
                    Config::LEN
                ),
            });
        }
    };
    let config = Config::parse(&cookie[..Config::LEN]).map_err(|e| Error::Unsupported {
        what: format!("mp4: {e}"),
    })?;
    let audio = &stream.audio;
    let channels = u16::from(config.channels);
    if channels != audio.layout().channels() {
        return Err(mismatch(&format!(
            "it says {channels} channels, the stream has {}",
            audio.layout().channels()
        )));
    }
    if config.sample_rate != audio.sample_rate() {
        return Err(mismatch(&format!(
            "it says a rate of {} Hz, the stream has {}",
            config.sample_rate,
            audio.sample_rate()
        )));
    }
    // What the reader will make of the depth; the stream must be that, or it comes back changed.
    let (format, raw) = match config.bit_depth {
        16 => (SampleFormat::S16, None),
        20 => (SampleFormat::S24, Some(20)),
        24 => (SampleFormat::S24, None),
        _ => (SampleFormat::S32, None),
    };
    let stream_raw = stream
        .bits_per_raw_sample
        .filter(|&b| usize::from(b) != audio.format().bits());
    if audio.format() != format || stream_raw != raw {
        return Err(mismatch(&format!(
            "it says a bit depth of {}, the stream is {:?} with {:?} significant bits",
            config.bit_depth,
            audio.format(),
            stream.bits_per_raw_sample
        )));
    }
    // The reader gives ALAC's own layout for the count, so no other layout can come back.
    let alac = crossfade_alac::wave_layout(config.channels)
        .and_then(|w| ChannelLayout::from_mask(w.mask, channels));
    if alac.as_ref() != Some(audio.layout()) {
        return Err(Error::Unsupported {
            what: format!(
                "mp4: alac stores only its own channel layout for {channels} channels, \
                 not {:?}",
                audio.layout()
            ),
        });
    }
    let inner = full(b"alac", 0, 0, &[&cookie[..Config::LEN]])?;
    // Mono and stereo are what a reader assumes; more channels need their layout said.
    let chan = match (chan_atom, alac_layout_tag(config.channels)) {
        (Some(atom), _) => atom.to_vec(),
        (None, Some(tag)) => full(b"chan", 0, 0, &[&tag.to_be_bytes(), &[0; 8]])?,
        (None, None) => Vec::new(),
    };
    sound(
        b"alac",
        channels,
        u16::from(config.bit_depth),
        audio.sample_rate(),
        &[inner, chan].concat(),
    )
}
