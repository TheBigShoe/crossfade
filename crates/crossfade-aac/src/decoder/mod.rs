//! The AAC-LC decoder (ISO/IEC 14496-3 subpart 4, which MPEG-2 AAC LC shares).
//!
//! Implemented: SCE, CPE (common window, M/S), LFE, DSE and FIL (skipped; an SBR or PS
//! payload in a FIL is noticed, see [`Decoder::sbr_present`], and the stream plays as its AAC
//! core), PCE (skipped in a block; a configuration's own PCE maps the elements), END; all four
//! window sequences and both window shapes; Huffman books 1-11 with escapes; pulse data;
//! intensity stereo; noise substitution (a fixed-seed generator, so output is reproducible);
//! TNS for long and short windows. Refused by name: coupling channel elements, Main-profile
//! prediction, LTP, SSR gain control, 960-sample frames, other object types.
//!
//! Output order is the WAVE channel mask's for channel configurations 1 to 7 (see
//! [`Decoder::channel_mask`]):
//!
//! | config | elements in the stream | output |
//! |---|---|---|
//! | 1 | C | C |
//! | 2 | L R | L R |
//! | 3 | C, L R | L R C |
//! | 4 | C, L R, Cs | L R C Cs (back centre) |
//! | 5 | C, L R, Ls Rs | L R C Ls Rs (side) |
//! | 6 | C, L R, Ls Rs, LFE | L R C LFE Ls Rs (side) |
//! | 7 | C, Lc Rc, L R, Ls Rs, LFE | L R C LFE Lc Rc Ls Rs (side) |
//! | 11 | C, L R, Ls Rs, Cs, LFE | L R C LFE Cs (back centre) Ls Rs (side) |
//!
//! With a program config element, channels come in its order (front, side, back elements,
//! then LFEs), unpositioned.

use crate::Result;
use crate::asc::{AudioSpecificConfig, ProgramConfig};
use crate::bits::BitReader;
use crate::error::{malformed, unsupported};

pub(crate) mod codebooks;
mod filterbank;
mod huffman;
mod ics;
pub(crate) mod tables;
mod tools;

use filterbank::{ONLY_LONG, Overlap, Scratch, synthesize};
use ics::{Ics, IcsInfo, MAX_SFB};
use tools::{MsMask, Noise};

/// Samples per channel in a frame.
pub const FRAME_LEN: usize = 1024;

/// Output channels at most (a program config element can name up to 93).
pub const MAX_CHANNELS: usize = 64;

const ID_SCE: u32 = 0;
const ID_CPE: u32 = 1;
const ID_CCE: u32 = 2;
const ID_LFE: u32 = 3;
const ID_DSE: u32 = 4;
const ID_PCE: u32 = 5;
const ID_FIL: u32 = 6;
const ID_END: u32 = 7;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Single,
    Pair,
    Lfe,
}

/// Where an element's channels go: the n-th element of its kind (by order in the block, for a
/// channel configuration) or the element with its tag (for a program config element).
struct Slot {
    kind: Kind,
    tag: Option<u8>,
    outputs: [usize; 2],
}

/// Decodes raw data blocks into interleaved `f32`, nominally -1..1.
pub struct Decoder {
    sample_rate: u32,
    sampling_index: usize,
    channels: usize,
    mask: Option<u32>,
    slots: Vec<Slot>,
    sbr_signalled: bool,
    sbr_seen: bool,
    overlap: Vec<Overlap>,
    /// Each output channel's spectrum, window sequence and shape for the frame being decoded,
    /// and whether the block held it.
    frame: Vec<(Vec<f32>, u8, u8)>,
    decoded: Vec<bool>,
    left: Ics,
    right: Ics,
    ms: MsMask,
    noise: Noise,
    scratch: Scratch,
    /// The frame's output, one channel after another, before interleaving.
    planar: Vec<f32>,
}

impl Decoder {
    pub fn new(config: &AudioSpecificConfig) -> Result<Self> {
        match config.object_type {
            2 => {}
            1 => return Err(unsupported("AAC Main (prediction) is not supported")),
            3 => return Err(unsupported("AAC SSR (gain control) is not supported")),
            4 => {
                return Err(unsupported(
                    "AAC LTP (long term prediction) is not supported",
                ));
            }
            t => {
                return Err(unsupported(format!(
                    "audio object type {t} is not supported: only AAC LC is"
                )));
            }
        }
        if config.frame_length_flag {
            return Err(unsupported(
                "960-sample frames (frameLengthFlag) are not supported",
            ));
        }
        let sampling_index = usize::from(config.sampling_index);
        if sampling_index > 11 {
            return Err(unsupported(format!(
                "sampling rate {} Hz is not supported (8 to 96 kHz)",
                config.sample_rate
            )));
        }
        let (slots, mask) = match &config.program_config {
            Some(pce) if config.channel_configuration == 0 => (pce_slots(pce), None),
            _ => {
                let (slots, mask) = config_slots(config.channel_configuration)
                    .ok_or_else(|| malformed("channel configuration 0 without a PCE"))?;
                (slots, Some(mask))
            }
        };
        let channels = slots
            .iter()
            .map(|s| if s.kind == Kind::Pair { 2 } else { 1 })
            .sum::<usize>();
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(unsupported(format!(
                "{channels} channels (1 to {MAX_CHANNELS} are)"
            )));
        }
        Ok(Self {
            sample_rate: config.sample_rate,
            sampling_index,
            channels,
            mask,
            slots,
            sbr_signalled: config.sbr.is_some(),
            sbr_seen: false,
            overlap: (0..channels).map(|_| Overlap::new()).collect(),
            frame: (0..channels)
                .map(|_| (vec![0.0; FRAME_LEN], 0, 0))
                .collect(),
            decoded: vec![false; channels],
            left: Ics::new(),
            right: Ics::new(),
            ms: MsMask {
                present: 0,
                used: [[false; MAX_SFB]; 8],
            },
            noise: Noise::new(),
            scratch: Scratch::new(),
            planar: vec![0.0; FRAME_LEN * channels],
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The WAVE channel mask the output order follows; `None` for a program configuration,
    /// whose channels are unpositioned.
    pub fn channel_mask(&self) -> Option<u32> {
        self.mask
    }

    /// Whether the stream is HE-AAC: SBR signalled in the configuration, or an SBR payload
    /// seen in a fill element. Only the AAC core is decoded, at the core rate.
    pub fn sbr_present(&self) -> bool {
        self.sbr_signalled || self.sbr_seen
    }

    /// Decodes one raw data block into `out` (cleared first): [`FRAME_LEN`] interleaved frames
    /// of [`channels`](Self::channels) samples. Returns the frame count. Bytes after the
    /// block's end are ignored.
    pub fn decode(&mut self, raw_data_block: &[u8], out: &mut Vec<f32>) -> Result<usize> {
        self.decode_blocks(raw_data_block, 1, out)
    }

    /// Decodes `blocks` raw data blocks that follow one another in `data`, each starting on a
    /// byte boundary (an ADTS frame of several blocks without a CRC, whose block boundaries
    /// only decoding finds), into `out` (cleared first). Returns the frame count.
    pub fn decode_blocks(
        &mut self,
        data: &[u8],
        blocks: usize,
        out: &mut Vec<f32>,
    ) -> Result<usize> {
        let mut r = BitReader::new(data);
        out.clear();
        for i in 0..blocks {
            if i > 0 {
                r.byte_align();
            }
            self.decode_block(&mut r, out)?;
        }
        Ok(blocks * FRAME_LEN)
    }

    /// Decodes the block at the reader, appending its frames to `out`.
    fn decode_block(&mut self, r: &mut BitReader<'_>, out: &mut Vec<f32>) -> Result<()> {
        let r = &mut *r;
        self.decoded.fill(false);
        let mut seen = [0usize; 3];
        loop {
            let id = r.read(3);
            match id {
                ID_SCE | ID_LFE => {
                    let tag = r.read(4) as u8;
                    let kind = if id == ID_SCE {
                        Kind::Single
                    } else {
                        Kind::Lfe
                    };
                    let slot = self.slot(kind, tag, &mut seen)?;
                    self.left.read(r, None, self.sampling_index)?;
                    if kind == Kind::Lfe && self.left.info.is_short() {
                        return Err(malformed("an LFE channel with short windows"));
                    }
                    tools::pns(&mut self.noise, &mut self.left, None);
                    tools::tns(&mut self.left, self.sampling_index);
                    self.keep(slot[0], true);
                }
                ID_CPE => {
                    let tag = r.read(4) as u8;
                    let slot = self.slot(Kind::Pair, tag, &mut seen)?;
                    self.read_pair(r)?;
                    self.keep(slot[0], true);
                    self.keep(slot[1], false);
                }
                ID_CCE => {
                    return Err(unsupported("coupling channel elements are not supported"));
                }
                ID_DSE => {
                    r.skip(4);
                    let align = r.bit();
                    let mut count = r.read(8) as usize;
                    if count == 255 {
                        count += r.read(8) as usize;
                    }
                    if align {
                        r.byte_align();
                    }
                    r.skip(count * 8);
                }
                ID_PCE => {
                    ProgramConfig::read(r)?;
                }
                ID_FIL => {
                    let mut count = r.read(4) as usize;
                    if count == 15 {
                        count += r.read(8) as usize;
                        count -= 1;
                    }
                    if count > 0 {
                        // extension_type: EXT_SBR_DATA or EXT_SBR_DATA_CRC.
                        let kind = r.peek32() >> 28;
                        if kind == 13 || kind == 14 {
                            self.sbr_seen = true;
                        }
                    }
                    r.skip(count * 8);
                }
                _ => {
                    debug_assert_eq!(id, ID_END);
                    break;
                }
            }
            if r.overrun() {
                return Err(malformed("raw data block ends inside an element"));
            }
        }
        let planar = self.planar.as_chunks_mut::<FRAME_LEN>().0;
        for (ch, pcm) in planar.iter_mut().enumerate() {
            let (spec, seq, shape) = &mut self.frame[ch];
            if !self.decoded[ch] {
                // A channel the block left out: silence, still overlapping the last frame.
                spec.fill(0.0);
                (*seq, *shape) = (ONLY_LONG, self.overlap[ch].shape);
            }
            synthesize(
                spec,
                *seq,
                *shape,
                &mut self.overlap[ch],
                &mut self.scratch,
                pcm,
            );
        }
        interleave(planar, out);
        Ok(())
    }

    /// Forgets all stream state, as if new.
    pub fn reset(&mut self) {
        for o in &mut self.overlap {
            o.reset();
        }
        self.noise.reset();
        self.sbr_seen = false;
    }

    /// The outputs of the next element of `kind` with `tag`.
    fn slot(&self, kind: Kind, tag: u8, seen: &mut [usize; 3]) -> Result<[usize; 2]> {
        let n = &mut seen[kind as usize];
        let found = if self.mask.is_some() {
            self.slots.iter().filter(|s| s.kind == kind).nth(*n)
        } else {
            self.slots
                .iter()
                .find(|s| s.kind == kind && s.tag == Some(tag))
        };
        *n += 1;
        found.map(|s| s.outputs).ok_or_else(|| {
            malformed(format!(
                "{kind:?} element (tag {tag}) not in the configuration"
            ))
        })
    }

    fn read_pair(&mut self, r: &mut BitReader<'_>) -> Result<()> {
        let common = r.bit();
        let mut info = None;
        self.ms.present = 0;
        if common {
            let i = IcsInfo::read(r, self.sampling_index)?;
            self.ms.present = r.read(2) as u8;
            if self.ms.present == 3 {
                return Err(malformed("reserved ms_mask_present 3"));
            }
            if self.ms.present == 1 {
                for g in 0..i.num_groups {
                    for b in 0..i.max_sfb {
                        self.ms.used[g][b] = r.bit();
                    }
                }
            }
            info = Some(i);
        }
        self.left.read(r, info, self.sampling_index)?;
        self.right.read(r, info, self.sampling_index)?;
        tools::pns(&mut self.noise, &mut self.left, None);
        let ms = if common { Some(&self.ms) } else { None };
        tools::pns(
            &mut self.noise,
            &mut self.right,
            ms.map(|m| (&self.left, m)),
        );
        if common {
            tools::ms_stereo(&self.ms, &mut self.left, &mut self.right);
        }
        let none = MsMask {
            present: 0,
            used: [[false; MAX_SFB]; 8],
        };
        tools::intensity_stereo(ms.unwrap_or(&none), &self.left, &mut self.right);
        tools::tns(&mut self.left, self.sampling_index);
        tools::tns(&mut self.right, self.sampling_index);
        Ok(())
    }

    /// Keeps the left (or right) element's spectrum for output channel `ch`.
    fn keep(&mut self, ch: usize, left: bool) {
        let ics = if left {
            &mut self.left
        } else {
            &mut self.right
        };
        let (seq, shape) = (ics.info.window_sequence, ics.info.window_shape);
        let (spec, s, w) = &mut self.frame[ch];
        // A swap, not a copy: reading an element rewrites its whole spectrum first.
        std::mem::swap(spec, &mut ics.spec);
        (*s, *w) = (seq, shape);
        self.decoded[ch] = true;
    }
}

/// Appends the channels of `planar` to `out`, interleaved.
fn interleave(planar: &[[f32; FRAME_LEN]], out: &mut Vec<f32>) {
    match planar {
        [mono] => out.extend_from_slice(mono),
        // The common case gets a loop that vectorises.
        [left, right] => {
            let base = out.len();
            out.resize(base + 2 * FRAME_LEN, 0.0);
            let pairs = out[base..].as_chunks_mut::<2>().0;
            for ((o, &l), &r) in pairs.iter_mut().zip(left).zip(right) {
                *o = [l, r];
            }
        }
        _ => {
            let base = out.len();
            out.resize(base + FRAME_LEN * planar.len(), 0.0);
            for (ch, pcm) in planar.iter().enumerate() {
                let dst = out[base + ch..].iter_mut().step_by(planar.len());
                for (o, &s) in dst.zip(pcm) {
                    *o = s;
                }
            }
        }
    }
}

fn config_slots(config: u8) -> Option<(Vec<Slot>, u32)> {
    use Kind::*;
    let s = |kind, a, b| Slot {
        kind,
        tag: None,
        outputs: [a, b],
    };
    let (slots, mask) = match config {
        1 => (vec![s(Single, 0, 0)], 0x4),
        2 => (vec![s(Pair, 0, 1)], 0x3),
        3 => (vec![s(Single, 2, 0), s(Pair, 0, 1)], 0x7),
        4 => (vec![s(Single, 2, 0), s(Pair, 0, 1), s(Single, 3, 0)], 0x107),
        5 => (vec![s(Single, 2, 0), s(Pair, 0, 1), s(Pair, 3, 4)], 0x607),
        6 => (
            vec![s(Single, 2, 0), s(Pair, 0, 1), s(Pair, 4, 5), s(Lfe, 3, 0)],
            0x60F,
        ),
        7 => (
            vec![
                s(Single, 2, 0),
                s(Pair, 4, 5),
                s(Pair, 0, 1),
                s(Pair, 6, 7),
                s(Lfe, 3, 0),
            ],
            0x6CF,
        ),
        11 => (
            vec![
                s(Single, 2, 0),
                s(Pair, 0, 1),
                s(Pair, 5, 6),
                s(Single, 4, 0),
                s(Lfe, 3, 0),
            ],
            0x70F,
        ),
        _ => return None,
    };
    Some((slots, mask))
}

fn pce_slots(pce: &ProgramConfig) -> Vec<Slot> {
    let mut next = 0;
    let mut slots = Vec::new();
    for e in pce.front.iter().chain(&pce.side).chain(&pce.back) {
        let kind = if e.is_cpe { Kind::Pair } else { Kind::Single };
        slots.push(Slot {
            kind,
            tag: Some(e.tag),
            outputs: [next, next + usize::from(e.is_cpe)],
        });
        next += 1 + usize::from(e.is_cpe);
    }
    for &tag in &pce.lfe {
        slots.push(Slot {
            kind: Kind::Lfe,
            tag: Some(tag),
            outputs: [next, next],
        });
        next += 1;
    }
    slots
}
