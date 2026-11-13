//! What an MPEG-4 AudioSpecificConfig says about the stream, through `crossfade_aac::asc`
//! (the parser alone, never the patent-encumbered decoder).

use super::boxes::malformed;
use crossfade_core::{ChannelLayout, Error};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AudioSpecificConfig {
    /// The core's audio object type (2 for AAC LC), under any SBR or PS signalling.
    pub(super) object_type: u8,
    /// The core's sampling rate: SBR doubles it on output, which is the decoder's to say.
    pub(super) sample_rate: u32,
    pub(super) layout: ChannelLayout,
    /// SBR or PS signalling was present (explicit or backward-compatible).
    pub(super) sbr: bool,
    pub(super) ps: bool,
}

/// The AAC layout of a channel configuration, as WAVE positions in storage order: the
/// configurations order channels C, L R, then surrounds, then LFE (14496-3, table 1.19); 7 is
/// 7.1 with front centre-left and centre-right. The surrounds are the side pair, as for ALAC,
/// whose layouts copy AAC's. The decoder emits this order.
pub(super) fn layout(config: u8) -> Result<ChannelLayout, Error> {
    let (mask, count) = match config {
        1 => (0x4, 1),
        2 => (0x3, 2),
        3 => (0x7, 3),
        4 => (0x107, 4),
        5 => (0x607, 5),
        6 => (0x60F, 6),
        7 => (0x6CF, 8),
        n => {
            return Err(Error::Unsupported {
                what: format!("mp4: aac channel configuration {n} is not supported"),
            });
        }
    };
    ChannelLayout::from_mask(mask, count).ok_or_else(|| malformed("bad channel layout"))
}

pub(super) fn parse(data: &[u8]) -> Result<AudioSpecificConfig, Error> {
    use crossfade_aac::Error as AacError;
    let a = crossfade_aac::asc::AudioSpecificConfig::parse(data).map_err(|e| match e {
        AacError::UnexpectedEnd => malformed("AudioSpecificConfig is cut off"),
        AacError::Unsupported(what) => Error::Unsupported { what },
        other => malformed(other.to_string()),
    })?;
    let layout = match &a.program_config {
        // A program configuration's channels come in its own order, unpositioned.
        Some(_) if a.channel_configuration == 0 => ChannelLayout::unpositioned(a.channels())?,
        _ => layout(a.channel_configuration)?,
    };
    Ok(AudioSpecificConfig {
        object_type: a.object_type,
        sample_rate: a.sample_rate,
        layout,
        sbr: a.sbr.is_some(),
        ps: a.sbr.is_some_and(|s| s.ps),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unsupported(r: Result<AudioSpecificConfig, Error>) -> String {
        match r {
            Err(Error::Unsupported { what }) => what,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn plain_aac_lc() {
        // What afconvert wrote: LC, index 4 (44.1 kHz), stereo.
        let a = parse(&[0x12, 0x10]).unwrap();
        assert_eq!(
            a,
            AudioSpecificConfig {
                object_type: 2,
                sample_rate: 44100,
                layout: ChannelLayout::stereo(),
                sbr: false,
                ps: false,
            }
        );
        // LC, 48 kHz, mono; 5.1 at 32 kHz.
        assert_eq!(parse(&[0x11, 0x88]).unwrap().layout, ChannelLayout::mono());
        let six = parse(&[0x12, 0xB0]).unwrap();
        assert_eq!(six.sample_rate, 32000);
        assert_eq!(six.layout, ChannelLayout::from_name("5.1").unwrap());
    }

    #[test]
    fn explicit_sbr_and_ps_report_the_core() {
        // Object type 5 (SBR), core index 6 (24 kHz), stereo; extension index 3 (48 kHz); then
        // the core type 2: 00101 0110 0010 0011 00010 = 0x2B 0x11 0x88 (padded).
        // The GASpecificConfig's three zero bits follow, into a fourth byte.
        let he = parse(&[0x2B, 0x11, 0x88, 0x00]).unwrap();
        assert_eq!(
            (he.object_type, he.sample_rate, he.sbr, he.ps),
            (2, 24000, true, false)
        );
        assert_eq!(he.layout, ChannelLayout::stereo());
        // Type 29 (PS), mono at 24 kHz, extension 48 kHz, core 2.
        // 11101 0110 0001 0011 00010 -> 0xEB 0x09 0x88.
        let ps = parse(&[0xEB, 0x09, 0x88, 0x00]).unwrap();
        assert_eq!((ps.sample_rate, ps.sbr, ps.ps), (24000, true, true));
        assert_eq!(ps.layout, ChannelLayout::mono());
    }

    #[test]
    fn an_explicit_rate_and_an_object_type_without_a_general_audio_config() {
        // LC, rate escape 15 then 24 bits (22000), channels 1, GASpecificConfig 000:
        // 00010 1111 000000000101010111110000 0001 000
        let bits = "00010111100000000010101011111000000010000";
        let mut bytes = vec![0u8; bits.len().div_ceil(8)];
        for (i, c) in bits.bytes().enumerate() {
            if c == b'1' {
                bytes[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let a = parse(&bytes).unwrap();
        assert_eq!((a.object_type, a.sample_rate), (2, 22000));
        // Type 31 + 6 bits (0 -> 32, MPEG-1 Layer-1) has no AAC configuration to describe.
        assert_eq!(
            unsupported(parse(&[0xF8, 0x12, 0x10])),
            "aac: audio object type 32 (Layer-1) is not supported"
        );
    }

    #[test]
    fn a_program_config_element_gives_unpositioned_channels() {
        // LC, 44.1 kHz, configuration 0, GASpecificConfig 000, then a PCE of one front pair
        // and one LFE: tag 0000, profile 01, index 0100, front 0001, side 0000, back 0000,
        // lfe 01, assoc 000, cc 0000, flags 000, CPE 1 0000, lfe 0000, align, comment 0.
        let bits = [
            "00010", "0100", "0000", "000", // LC, 44.1 kHz, configuration 0, GASpecificConfig
            "0000", "01", "0100", "0001", "0000", "0000", "01", "000", "0000", "000", "1", "0000",
            "0000", "00000", // then the byte alignment
            "00000000",
        ]
        .concat();
        let mut bytes = vec![0u8; bits.len().div_ceil(8)];
        for (i, c) in bits.bytes().enumerate() {
            if c == b'1' {
                bytes[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let a = parse(&bytes).unwrap();
        assert_eq!(a.layout, ChannelLayout::unpositioned(3).unwrap());
    }

    #[test]
    fn what_it_refuses() {
        // Configuration 0 with no program config element after it.
        assert!(
            parse(&[0x12, 0x00])
                .unwrap_err()
                .to_string()
                .contains("cut off")
        );
        assert!(unsupported(parse(&[0x12, 0x40])).contains("channel configuration 8"));
        let short = parse(&[0x12]).unwrap_err();
        assert!(short.to_string().contains("cut off"), "{short}");
        // Index 13 is reserved.
        assert!(
            parse(&[0x16, 0x90])
                .unwrap_err()
                .to_string()
                .contains("reserved")
        );
        // An explicit rate of 0.
        assert!(parse(&[0x17, 0x80, 0x00, 0x00, 0x10]).is_err());
        let seven = parse(&[0x12, 0x38]).unwrap();
        assert_eq!(seven.layout.channels(), 8);
        // Configuration 11 is 6.1 (WAVE mask 0x70F), as afconvert writes it.
        assert_eq!(
            parse(&[0x11, 0xD8]).unwrap().layout,
            ChannelLayout::from_mask(0x70F, 7).unwrap()
        );
    }
}
