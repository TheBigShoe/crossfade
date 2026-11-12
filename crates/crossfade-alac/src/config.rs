//! The ALACSpecificConfig ("magic cookie"): 24 big-endian bytes that describe a stream.

use crate::{Error, Result};

/// The longest frame this crate decodes. Apple's encoder writes 4096; the format's field is 32
/// bits, and a decoder's buffers are a few times `frame_length * channels` samples.
pub const MAX_FRAME_LENGTH: u32 = 1 << 20;

/// A stream's parameters, as the ALACSpecificConfig gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Sample frames in every packet but a shorter last one.
    pub frame_length: u32,
    pub compatible_version: u8,
    /// 16, 20, 24 or 32.
    pub bit_depth: u8,
    /// The adaptive Rice coder's tuning: history mult, initial history, and limit.
    pub pb: u8,
    pub mb: u8,
    pub kb: u8,
    /// 1 to 8.
    pub channels: u8,
    pub max_run: u16,
    /// 0 when unknown.
    pub max_frame_bytes: u32,
    /// 0 when unknown.
    pub avg_bit_rate: u32,
    pub sample_rate: u32,
}

impl Config {
    /// Bytes in the bare config.
    pub const LEN: usize = 24;

    /// Reads and checks a config. Takes the 24 bytes alone or with the wrappings containers
    /// put around them: an `alac` atom header with its version and flags (MP4 and QuickTime
    /// sample descriptions), and before it a `frma` atom (QuickTime's `wave` box). Bytes after
    /// the config (a `chan` atom in CAF) are ignored.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let _ = bytes; Err(Error::InvalidConfig("not built yet"))
    }

    /// The 24 bytes `parse` reads.
    pub fn write(&self) -> [u8; Self::LEN] {
        [0; Self::LEN]
    }

    /// What a decoder needs to hold: the fields it reads, in range.
    pub fn check(&self) -> Result<()> {
        Ok(())
    }
}

/// How ALAC's channels map onto WAVE speaker positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveLayout {
    /// The WAVE channel mask (`dwChannelMask`): the positions, in bit order.
    pub mask: u32,
    /// For each channel in WAVE order, the index of the ALAC channel that carries it.
    pub from_alac: &'static [u8],
}

/// ALAC's fixed layout for `channels` (1 to 8) as WAVE positions.
///
/// ALAC orders channels by its own layout for each count (`ALACMagicCookieDescription.txt`,
/// "Channel Layout", and `ALACAudioTypes.h`): mono C; stereo L R; 3.0 C L R; 4.0 C L R Cs;
/// 5.0 C L R Ls Rs; 5.1 C L R Ls Rs LFE; 6.1 C L R Ls Rs Cs LFE; 7.1 C Lc Rc L R Ls Rs LFE.
/// Core Audio's surround labels Ls/Rs are the WAVE side pair (macOS `afconvert` turns a WAVE
/// mask of 0x60F into "L R C LFE Ls Rs" and 0x3F into "L R C LFE Rls Rrs"), Cs is back
/// center and Lc/Rc are front left and right of center.
pub fn wave_layout(channels: u8) -> Option<WaveLayout> {
        let _ = channels; None
    }

#[cfg(test)]
mod tests {
    use super::*;

    /// What `afconvert -d alac` wrote for 16-bit stereo at 44.1 kHz.
    const AFCONVERT: [u8; 24] = [
        0x00, 0x00, 0x10, 0x00, 0x00, 0x10, 0x28, 0x0a, 0x0e, 0x02, 0x00, 0xff, 0x00, 0x00, 0x27,
        0xed, 0x00, 0x0a, 0xf7, 0xf3, 0x00, 0x00, 0xac, 0x44,
    ];

    fn afconvert() -> Config {
        Config {
            frame_length: 4096,
            compatible_version: 0,
            bit_depth: 16,
            pb: 40,
            mb: 10,
            kb: 14,
            channels: 2,
            max_run: 255,
            max_frame_bytes: 0x27ed,
            avg_bit_rate: 0x000a_f7f3,
            sample_rate: 44100,
        }
    }

    #[test]
    fn reads_the_fields_in_order() {
        assert_eq!(Config::parse(&AFCONVERT), Ok(afconvert()));
        assert_eq!(afconvert().write(), AFCONVERT);
    }

    #[test]
    fn accepts_the_atom_wrappings_and_ignores_what_follows() {
        let mut atom = vec![0, 0, 0, 36];
        atom.extend_from_slice(b"alac");
        atom.extend_from_slice(&[0; 4]);
        atom.extend_from_slice(&AFCONVERT);
        let mut frma = vec![0, 0, 0, 12];
        frma.extend_from_slice(b"frmaalac");
        frma.extend_from_slice(&atom);
        let mut chan = AFCONVERT.to_vec();
        chan.extend_from_slice(b"\0\0\0\x18chan and some more");
        for bytes in [&atom[..], &frma, &chan] {
            assert_eq!(Config::parse(bytes), Ok(afconvert()), "{bytes:?}");
        }
    }

    #[test]
    fn refuses_what_it_cannot_decode() {
        let with = |f: fn(&mut Config)| {
            let mut c = afconvert();
            f(&mut c);
            Config::parse(&c.write())
        };
        let bad = |what| Err(Error::InvalidConfig(what));
        assert_eq!(Config::parse(&AFCONVERT[..23]), bad("shorter than 24 bytes"));
        assert_eq!(
            with(|c| c.compatible_version = 1),
            bad("compatible version is not 0")
        );
        for n in [0, MAX_FRAME_LENGTH + 1, u32::MAX] {
            let mut c = afconvert();
            c.frame_length = n;
            assert_eq!(
                Config::parse(&c.write()),
                bad("frame length is not between 1 and 1048576")
            );
        }
        for depth in [0, 8, 15, 17, 25, 31, 33, 255] {
            let mut c = afconvert();
            c.bit_depth = depth;
            assert_eq!(
                Config::parse(&c.write()),
                bad("bit depth is not 16, 20, 24 or 32"),
                "{depth}"
            );
        }
        for n in [0, 9, 255] {
            let mut c = afconvert();
            c.channels = n;
            assert_eq!(
                Config::parse(&c.write()),
                bad("channel count is not between 1 and 8")
            );
        }
        for kb in [0, 32, 255] {
            let mut c = afconvert();
            c.kb = kb;
            assert_eq!(Config::parse(&c.write()), bad("kb is not between 1 and 31"));
        }
        for (depth, channels, frame_length) in [(20, 1, 1), (24, 8, MAX_FRAME_LENGTH), (32, 5, 4096)]
        {
            let mut c = afconvert();
            (c.bit_depth, c.channels, c.frame_length) = (depth, channels, frame_length);
            assert_eq!(Config::parse(&c.write()), Ok(c));
        }
    }

    #[test]
    fn every_layout_is_a_permutation_of_its_channels() {
        for n in 1..=8u8 {
            let l = wave_layout(n).unwrap();
            assert_eq!(l.mask.count_ones(), u32::from(n));
            let mut seen = l.from_alac.to_vec();
            seen.sort_unstable();
            assert_eq!(seen, (0..n).collect::<Vec<_>>());
        }
        assert_eq!(wave_layout(0), None);
        assert_eq!(wave_layout(9), None);
        // 5.1: C L R Ls Rs LFE becomes FL FR FC LFE SL SR.
        assert_eq!(
            wave_layout(6),
            Some(WaveLayout {
                mask: 0x60F,
                from_alac: &[1, 2, 0, 5, 3, 4]
            })
        );
    }
}
