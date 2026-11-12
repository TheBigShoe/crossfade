//! The first fields of an MPEG-4 AudioSpecificConfig (ISO/IEC 14496-3, 1.6.2.1): enough to
//! describe the stream. `crossfade-aac`'s parser replaces this one when that crate lands.

use super::boxes::malformed;
use crossfade_core::{ChannelLayout, Error};

/// Sampling frequencies by index (14496-3, table 1.18); 13 and 14 are reserved, 15 escapes.
const RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

const SBR: u8 = 5;
const PS: u8 = 29;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AudioSpecificConfig {
    /// The core's audio object type (2 for AAC LC), under any SBR or PS signalling.
    pub(super) object_type: u8,
    /// The core's sampling rate: SBR doubles it on output, which is the decoder's to say.
    pub(super) sample_rate: u32,
    pub(super) layout: ChannelLayout,
    /// Explicit (hierarchical) SBR or PS signalling was present.
    pub(super) sbr: bool,
    pub(super) ps: bool,
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: usize) -> Result<u32, Error> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self
                .data
                .get(self.pos / 8)
                .ok_or_else(|| malformed("AudioSpecificConfig is cut off"))?;
            v = v << 1 | u32::from(byte >> (7 - self.pos % 8) & 1);
            self.pos += 1;
        }
        Ok(v)
    }

    fn object_type(&mut self) -> Result<u8, Error> {
        let t = self.read(5)? as u8;
        Ok(if t == 31 { 32 + self.read(6)? as u8 } else { t })
    }

    fn rate(&mut self) -> Result<u32, Error> {
        match self.read(4)? as usize {
            15 => self.read(24),
            i => RATES
                .get(i)
                .copied()
                .ok_or_else(|| malformed(format!("sampling frequency index {i} is reserved"))),
        }
    }
}

/// The AAC layout of a channel configuration, as WAVE positions in storage order: the
/// configurations order channels C, L R, then surrounds, then LFE (14496-3, table 1.19); 7 is
/// 7.1 with front centre-left and centre-right. The surrounds are the side pair, as for ALAC,
/// whose layouts copy AAC's.
pub(super) fn layout(config: u8) -> Result<ChannelLayout, Error> {
    let (mask, count) = match config {
        1 => (0x4, 1),
        2 => (0x3, 2),
        3 => (0x7, 3),
        4 => (0x107, 4),
        5 => (0x607, 5),
        6 => (0x60F, 6),
        7 => (0x6CF, 8),
        0 => {
            return Err(Error::Unsupported {
                what: "mp4: aac whose channels a program config element gives (channel \
                       configuration 0) is not supported yet"
                    .into(),
            });
        }
        n => {
            return Err(Error::Unsupported {
                what: format!("mp4: aac channel configuration {n} is not supported"),
            });
        }
    };
    ChannelLayout::from_mask(mask, count).ok_or_else(|| malformed("bad channel layout"))
}

pub(super) fn parse(data: &[u8]) -> Result<AudioSpecificConfig, Error> {
    let mut b = Bits { data, pos: 0 };
    let mut object_type = b.object_type()?;
    let sample_rate = b.rate()?;
    let channels = b.read(4)? as u8;
    let (mut sbr, mut ps) = (false, false);
    if object_type == SBR || object_type == PS {
        sbr = true;
        ps = object_type == PS;
        let _extension_rate = b.rate()?;
        object_type = b.object_type()?;
    }
    if sample_rate == 0 {
        return Err(malformed("AudioSpecificConfig gives a sampling rate of 0"));
    }
    Ok(AudioSpecificConfig {
        object_type,
        sample_rate,
        layout: layout(channels)?,
        sbr,
        ps,
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
        let he = parse(&[0x2B, 0x11, 0x88]).unwrap();
        assert_eq!(
            (he.object_type, he.sample_rate, he.sbr, he.ps),
            (2, 24000, true, false)
        );
        assert_eq!(he.layout, ChannelLayout::stereo());
        // Type 29 (PS), mono at 24 kHz, extension 48 kHz, core 2.
        // 11101 0110 0001 0011 00010 -> 0xEB 0x09 0x88.
        let ps = parse(&[0xEB, 0x09, 0x88]).unwrap();
        assert_eq!((ps.sample_rate, ps.sbr, ps.ps), (24000, true, true));
        assert_eq!(ps.layout, ChannelLayout::mono());
    }

    #[test]
    fn escaped_type_and_explicit_rate() {
        // Type 31 + 6 bits (0 -> 32), rate escape 15 then 24 bits (22000), channels 1:
        // 11111 000000 1111 000000000101010111110000 0001
        let bits = "1111100000011110000000001010101111100000001";
        let mut bytes = vec![0u8; bits.len().div_ceil(8)];
        for (i, c) in bits.bytes().enumerate() {
            if c == b'1' {
                bytes[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let a = parse(&bytes).unwrap();
        assert_eq!((a.object_type, a.sample_rate), (32, 22000));
    }

    #[test]
    fn what_it_refuses() {
        assert!(unsupported(parse(&[0x12, 0x00])).contains("channel configuration 0"));
        assert!(unsupported(parse(&[0x12, 0x40])).contains("channel configuration 8"));
        let short = parse(&[0x12]).unwrap_err();
        assert!(short.to_string().contains("cut off"), "{short}");
        // Index 13 is reserved.
        assert!(parse(&[0x16, 0x90]).unwrap_err().to_string().contains("reserved"));
        // An explicit rate of 0.
        assert!(parse(&[0x17, 0x80, 0x00, 0x00, 0x10]).is_err());
        let seven = parse(&[0x12, 0x38]).unwrap();
        assert_eq!(seven.layout.channels(), 8);
    }
}
