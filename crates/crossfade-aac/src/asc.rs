//! The AudioSpecificConfig (ISO/IEC 14496-3 1.6.2.1) with its GASpecificConfig (4.4.1), and
//! the program config element (4.4.1.1) that a configuration or a raw data block may carry.

use crate::Error;
use crate::bits::BitReader;
use crate::error::{Result, malformed, unsupported};

/// SBR (and PS) signalled in a configuration. The decoder plays only the AAC core: this tells
/// a caller that the stream is HE-AAC and what the full rate would be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sbr {
    /// Signalled explicitly (object type 5 or 29 first) rather than by the backward-compatible
    /// extension after the core configuration.
    pub explicit: bool,
    /// Parametric stereo signalled too (HE-AAC v2).
    pub ps: bool,
    /// The output rate with SBR applied.
    pub sample_rate: u32,
}

/// One parsed AudioSpecificConfig.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpecificConfig {
    /// The core's audio object type (2 is AAC LC), after explicit SBR signalling is unwrapped.
    pub object_type: u8,
    /// The core's sampling rate in Hz.
    pub sample_rate: u32,
    /// The table index for the rate: as coded, or for an explicit rate the index whose tables
    /// the standard assigns to it (14496-3 Table 4.82).
    pub sampling_index: u8,
    /// 0 (the program config element says) or 1 to 7.
    pub channel_configuration: u8,
    pub program_config: Option<ProgramConfig>,
    /// 960-sample frames instead of 1024.
    pub frame_length_flag: bool,
    pub depends_on_core_coder: bool,
    pub extension_flag: bool,
    pub sbr: Option<Sbr>,
}

impl AudioSpecificConfig {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = BitReader::new(bytes);
        let mut object_type = object_type(&mut r);
        let (sample_rate, mut sampling_index) = rate(&mut r)?;
        let channel_configuration = r.read(4) as u8;
        let mut sbr = None;
        if object_type == 5 || object_type == 29 {
            let (ext_rate, _) = rate(&mut r)?;
            sbr = Some(Sbr {
                explicit: true,
                ps: object_type == 29,
                sample_rate: ext_rate,
            });
            object_type = self::object_type(&mut r);
            if object_type == 22 {
                r.skip(4);
            }
        }
        if r.overrun() {
            return Err(Error::UnexpectedEnd);
        }
        if !matches!(object_type, 1..=4 | 6 | 7 | 17 | 19..=23) {
            return Err(unsupported(format!(
                "audio object type {object_type} ({}) is not supported",
                object_type_name(object_type)
            )));
        }
        if channel_configuration > 7 {
            return Err(unsupported(format!(
                "channel configuration {channel_configuration} is not supported"
            )));
        }
        let frame_length_flag = r.bit();
        let depends_on_core_coder = r.bit();
        if depends_on_core_coder {
            r.skip(14);
        }
        let extension_flag = r.bit();
        let program_config = if channel_configuration == 0 {
            Some(ProgramConfig::read(&mut r)?)
        } else {
            None
        };
        if object_type == 6 || object_type == 20 {
            r.skip(3);
        }
        if extension_flag {
            if object_type == 22 {
                r.skip(16);
            }
            if matches!(object_type, 17 | 19 | 20 | 23) {
                r.skip(3);
            }
            r.skip(1);
        }
        if matches!(object_type, 17 | 19..=23) {
            r.skip(2); // epConfig
        }
        if r.overrun() {
            return Err(Error::UnexpectedEnd);
        }
        if sbr.is_none() && r.remaining() >= 16 && r.read(11) == 0x2B7 {
            let ext = self::object_type(&mut r);
            if ext == 5 && r.bit() {
                let (ext_rate, _) = rate(&mut r)?;
                let ps = r.remaining() >= 12 && r.read(11) == 0x548 && r.bit();
                if !r.overrun() {
                    sbr = Some(Sbr {
                        explicit: false,
                        ps,
                        sample_rate: ext_rate,
                    });
                }
            }
        }
        if sampling_index == 15 {
            sampling_index = index_for_rate(sample_rate);
        }
        if sample_rate == 0 {
            return Err(malformed("explicit sampling rate of 0 Hz"));
        }
        Ok(Self {
            object_type,
            sample_rate,
            sampling_index,
            channel_configuration,
            program_config,
            frame_length_flag,
            depends_on_core_coder,
            extension_flag,
            sbr,
        })
    }

    /// Output channels: from the channel configuration, or the program config element.
    pub fn channels(&self) -> u16 {
        match (self.channel_configuration, &self.program_config) {
            (0, Some(pce)) => pce.channels(),
            (7, _) => 8,
            (c, _) => u16::from(c),
        }
    }
}

/// A front, side or back channel element of a program configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element {
    /// A channel pair (two channels) rather than a single channel.
    pub is_cpe: bool,
    pub tag: u8,
}

/// A coupling channel element named by a program configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coupling {
    pub independently_switched: bool,
    pub tag: u8,
}

/// A program config element: which elements make up the program, in output order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramConfig {
    pub element_instance_tag: u8,
    /// The object type minus one, as in ADTS.
    pub profile: u8,
    pub sampling_index: u8,
    pub front: Vec<Element>,
    pub side: Vec<Element>,
    pub back: Vec<Element>,
    pub lfe: Vec<u8>,
    pub assoc_data: Vec<u8>,
    pub coupling: Vec<Coupling>,
    pub comment: Vec<u8>,
}

impl ProgramConfig {
    /// The PCE that opens a raw data block (`payload` starts at the block), if its first
    /// element is one; how an ADTS stream with channel configuration 0 says what it holds.
    pub fn from_raw_data_block(payload: &[u8]) -> Result<Option<Self>> {
        let mut r = BitReader::new(payload);
        if r.read(3) != 5 {
            return Ok(None);
        }
        Self::read(&mut r).map(Some)
    }

    /// Output channels: every front, side and back element (two for a pair) and every LFE.
    pub fn channels(&self) -> u16 {
        let count = |v: &[Element]| v.iter().map(|e| 1 + u16::from(e.is_cpe)).sum::<u16>();
        count(&self.front) + count(&self.side) + count(&self.back) + self.lfe.len() as u16
    }

    /// Reads the element after its 3-bit id. Byte alignment counts from the start of the
    /// reader's input, which is where the configuration or raw data block starts.
    pub(crate) fn read(r: &mut BitReader<'_>) -> Result<Self> {
        let element_instance_tag = r.read(4) as u8;
        let profile = r.read(2) as u8;
        let sampling_index = r.read(4) as u8;
        let (front, side, back) = (r.read(4), r.read(4), r.read(4));
        let (lfe, assoc, cc) = (r.read(2), r.read(3), r.read(4));
        if r.bit() {
            r.skip(4); // mono mixdown element
        }
        if r.bit() {
            r.skip(4); // stereo mixdown element
        }
        if r.bit() {
            r.skip(3); // matrix mixdown index and pseudo-surround
        }
        let elements = |r: &mut BitReader<'_>, n: u32| {
            (0..n)
                .map(|_| Element {
                    is_cpe: r.bit(),
                    tag: r.read(4) as u8,
                })
                .collect::<Vec<_>>()
        };
        let front = elements(r, front);
        let side = elements(r, side);
        let back = elements(r, back);
        let lfe = (0..lfe).map(|_| r.read(4) as u8).collect();
        let assoc_data = (0..assoc).map(|_| r.read(4) as u8).collect();
        let coupling = (0..cc)
            .map(|_| Coupling {
                independently_switched: r.bit(),
                tag: r.read(4) as u8,
            })
            .collect();
        r.byte_align();
        let comment_len = r.read(8);
        let comment = (0..comment_len).map(|_| r.read(8) as u8).collect();
        if r.overrun() {
            return Err(Error::UnexpectedEnd);
        }
        Ok(Self {
            element_instance_tag,
            profile,
            sampling_index,
            front,
            side,
            back,
            lfe,
            assoc_data,
            coupling,
            comment,
        })
    }
}

fn object_type(r: &mut BitReader<'_>) -> u8 {
    match r.read(5) as u8 {
        31 => 32 + r.read(6) as u8,
        t => t,
    }
}

/// The rate and its index (15 for an explicit rate).
fn rate(r: &mut BitReader<'_>) -> Result<(u32, u8)> {
    let index = r.read(4) as u8;
    match index {
        15 => Ok((r.read(24), 15)),
        13 | 14 => Err(malformed(format!("reserved sampling index {index}"))),
        _ => Ok((crate::adts::SAMPLE_RATES[usize::from(index)], index)),
    }
}

/// The index whose tables an explicit rate uses (14496-3 Table 4.82).
fn index_for_rate(rate: u32) -> u8 {
    const LOWER_BOUNDS: [u32; 11] = [
        92017, 75132, 55426, 46009, 37566, 27713, 23004, 18783, 13856, 11502, 9391,
    ];
    LOWER_BOUNDS.iter().position(|&b| rate >= b).unwrap_or(11) as u8
}

fn object_type_name(t: u8) -> &'static str {
    match t {
        0 => "null",
        8 => "CELP",
        9 => "HVXC",
        12 => "TTSI",
        13 => "Main synthesis",
        14 => "Wavetable synthesis",
        15 => "General MIDI",
        16 => "Algorithmic synthesis and audio effects",
        24 => "ER CELP",
        25 => "ER HVXC",
        26 => "ER HILN",
        27 => "ER Parametric",
        28 => "SSC",
        32 => "Layer-1",
        33 => "Layer-2",
        34 => "Layer-3",
        35 => "DST",
        36 => "ALS",
        37 => "SLS",
        38 => "SLS non-core",
        39 => "ER AAC ELD",
        40 => "SMR Simple",
        41 => "SMR Main",
        42 => "USAC",
        43 => "SAOC",
        44 => "LD MPEG Surround",
        45 => "USAC SAOC",
        _ => "reserved",
    }
}

#[cfg(test)]
mod tests;
