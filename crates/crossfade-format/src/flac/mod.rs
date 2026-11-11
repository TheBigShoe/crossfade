//! FLAC container. The reader and the writer live in separate files.

mod demux;
mod mux;

pub use demux::open;
pub use mux::create;

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Error, SampleFormat, StreamInfo, Tags, TimeBase,
};
use crossfade_flac::streaminfo::StreamInfoBlock;

/// The tag a FLAC file uses to say its channels differ from the defaults (RFC 9639 section
/// 8.6.2); its value is a WAVE channel mask.
pub(crate) const CHANNEL_MASK_TAG: &str = "WAVEFORMATEXTENSIBLE_CHANNEL_MASK";

/// FLAC's default channel orders (RFC 9639 section 9.1.3) as WAVE masks. The RFC's
/// "back/surround" pair for 5 and 6 channels is the side pair, which is what the reference
/// `flac` writes into a WAV for an untagged stream.
pub(crate) fn default_mask(channels: u16) -> Option<u32> {
    Some(match channels {
        1 => 0x4,
        2 => 0x3,
        3 => 0x7,
        4 => 0x33,
        5 => 0x607,
        6 => 0x60F,
        7 => 0x70F,
        8 => 0x63F,
        _ => return None,
    })
}

fn layout(channels: u8) -> Result<ChannelLayout, Error> {
    let n = u16::from(channels);
    default_mask(n)
        .and_then(|mask| ChannelLayout::from_mask(mask, n))
        .map_or_else(|| ChannelLayout::unpositioned(n), Ok)
}

/// The layout a channel mask tag names, if it has one that is valid for `channels`: hex with or
/// without `0x`, as many bits as channels, every bit a position we know.
fn tagged_layout(tags: &Tags, channels: u16) -> Option<ChannelLayout> {
    let value = tags.get(CHANNEL_MASK_TAG)?.trim();
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    // `from_str_radix` accepts a leading sign.
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    ChannelLayout::from_mask(u32::from_str_radix(digits, 16).ok()?, channels)
}

/// Replaces the default layout with the one the file's channel mask tag names; an invalid tag
/// changes nothing. Shared by the native and Ogg readers.
pub(crate) fn apply_channel_mask(stream: &mut StreamInfo, tags: &Tags) -> Result<(), Error> {
    let audio = &stream.audio;
    if let Some(layout) = tagged_layout(tags, audio.layout().channels()) {
        stream.audio = AudioSpec::new(audio.sample_rate(), audio.format(), layout)?;
    }
    Ok(())
}

/// The stream a STREAMINFO block describes; shared by the native and Ogg readers.
pub(crate) fn stream_info(block: &StreamInfoBlock, extradata: &[u8]) -> Result<StreamInfo, Error> {
    let bits = block.bits_per_sample;
    let format = match bits {
        0..=8 => SampleFormat::U8,
        9..=16 => SampleFormat::S16,
        17..=24 => SampleFormat::S24,
        _ => SampleFormat::S32,
    };
    let audio = AudioSpec::new(block.sample_rate, format, layout(block.channels)?)?;
    let time_base = TimeBase::new(1, i64::from(block.sample_rate))?;
    Ok(StreamInfo {
        frames: (block.total_samples != 0).then_some(block.total_samples),
        extradata: extradata.to_vec(),
        bits_per_raw_sample: (!matches!(bits, 8 | 16 | 24 | 32)).then_some(bits),
        ..StreamInfo::new(0, CodecId::Flac, audio, time_base)
    })
}

/// Maps the standalone FLAC crate's error onto the engine's, keeping its text.
pub(crate) fn engine_error(e: crossfade_flac::Error) -> Error {
    match e {
        crossfade_flac::Error::Malformed(what) => Error::Malformed { what },
        crossfade_flac::Error::Unsupported(what) => Error::Unsupported { what },
        crossfade_flac::Error::UnexpectedEof { offset } => Error::UnexpectedEof {
            offset: offset as u64,
        },
        // The engine has no variant for a bug; the text says what it was.
        other => Error::Unsupported {
            what: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{Channel, Channel::*, SampleFormat, Tags};

    #[test]
    fn an_internal_error_keeps_its_text() {
        assert_eq!(
            engine_error(crossfade_flac::Error::Internal("x".into())),
            Error::Unsupported {
                what: "internal error: x".into()
            }
        );
    }

    fn block(rate: u32, channels: u8, bits: u8, total: u64) -> StreamInfoBlock {
        StreamInfoBlock {
            min_block_size: 4096,
            max_block_size: 4096,
            min_frame_size: 0,
            max_frame_size: 0,
            sample_rate: rate,
            channels,
            bits_per_sample: bits,
            total_samples: total,
            md5: [0; 16],
        }
    }

    fn info(b: &StreamInfoBlock) -> StreamInfo {
        stream_info(b, &b.write()).unwrap()
    }

    #[test]
    fn basic_fields() {
        let b = block(44100, 2, 16, 1000);
        let s = info(&b);
        assert_eq!((s.index, s.codec), (0, CodecId::Flac));
        assert_eq!(s.time_base, TimeBase::new(1, 44100).unwrap());
        assert_eq!(s.frames, Some(1000));
        assert_eq!(s.extradata, b.write().to_vec());
        assert_eq!(s.audio.sample_rate(), 44100);
        assert_eq!(s.audio.layout(), &ChannelLayout::stereo());
        assert_eq!(s.bits_per_raw_sample, None);
    }

    #[test]
    fn an_unknown_total_is_none() {
        assert_eq!(info(&block(8000, 1, 16, 0)).frames, None);
    }

    #[test]
    fn depth_picks_the_smallest_container_and_records_odd_depths() {
        use SampleFormat::*;
        for (bits, format, raw) in [
            (4, U8, Some(4)),
            (8, U8, None),
            (9, S16, Some(9)),
            (12, S16, Some(12)),
            (16, S16, None),
            (17, S24, Some(17)),
            (20, S24, Some(20)),
            (24, S24, None),
            (25, S32, Some(25)),
            (32, S32, None),
        ] {
            let s = info(&block(48000, 2, bits, 1));
            assert_eq!(s.audio.format(), format, "{bits}");
            assert_eq!(s.bits_per_raw_sample, raw, "{bits}");
        }
    }

    #[test]
    fn channel_orders_follow_rfc_9639() {
        let positioned: [(u8, &[Channel]); 7] = [
            (1, &[FrontCenter]),
            (3, &[FrontLeft, FrontRight, FrontCenter]),
            (4, &[FrontLeft, FrontRight, BackLeft, BackRight]),
            (
                5,
                &[FrontLeft, FrontRight, FrontCenter, SideLeft, SideRight],
            ),
            (
                6,
                &[FrontLeft, FrontRight, FrontCenter, Lfe, SideLeft, SideRight],
            ),
            (
                7,
                &[
                    FrontLeft,
                    FrontRight,
                    FrontCenter,
                    Lfe,
                    BackCenter,
                    SideLeft,
                    SideRight,
                ],
            ),
            (
                8,
                &[
                    FrontLeft,
                    FrontRight,
                    FrontCenter,
                    Lfe,
                    BackLeft,
                    BackRight,
                    SideLeft,
                    SideRight,
                ],
            ),
        ];
        for (n, want) in positioned {
            let s = info(&block(48000, n, 16, 1));
            assert_eq!(s.audio.layout().positions(), Some(want), "{n}");
        }
        assert_eq!(info(&block(48000, 2, 16, 1)).audio.layout().channels(), 2);
    }

    #[test]
    fn the_extradata_is_passed_through_as_given() {
        let b = block(48000, 2, 16, 1);
        assert_eq!(stream_info(&b, &[1, 2, 3]).unwrap().extradata, [1, 2, 3]);
    }

    fn masked(channels: u8, value: &str) -> ChannelLayout {
        let mut stream = info(&block(48000, channels, 16, 1));
        let mut tags = Tags::new();
        tags.push("waveformatextensible_channel_mask", value);
        apply_channel_mask(&mut stream, &tags).unwrap();
        stream.audio.layout().clone()
    }

    #[test]
    fn a_channel_mask_tag_sets_the_layout() {
        let side = ChannelLayout::from_mask(0x060F, 6).unwrap();
        let back = ChannelLayout::from_mask(0x003F, 6).unwrap();
        // With or without the prefix, in either case, with spaces around it.
        for value in ["0x060F", "0X060f", "60F", " 0x60f "] {
            assert_eq!(masked(6, value), side, "{value:?}");
        }
        assert_eq!(masked(6, "0x003F"), back);
        assert_eq!(masked(1, "0x1").positions(), Some(&[FrontLeft][..]));
        assert_eq!(masked(2, "0x0C").positions(), Some(&[FrontCenter, Lfe][..]));
    }

    #[test]
    fn an_invalid_channel_mask_tag_is_ignored() {
        let default = info(&block(48000, 6, 16, 1)).audio.layout().clone();
        for value in [
            "",
            "0x",
            "zz",
            "0x3",
            "0x7FF",
            "0x1FFFFFFFF",
            "-1",
            "0x060F0000",
            "0x40000",
            "6.0",
        ] {
            assert_eq!(masked(6, value), default, "{value:?}");
        }
    }

    #[test]
    fn no_tag_leaves_the_default_layout() {
        let mut stream = info(&block(48000, 6, 16, 1));
        let before = stream.clone();
        apply_channel_mask(&mut stream, &Tags::new()).unwrap();
        assert_eq!(stream, before);
    }
}
