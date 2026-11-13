//! The Layer III frame decoder.

use crate::bits::Bits;
use crate::header::{ChannelMode, Layer};
use crate::requantize::GAIN_MIN;
use crate::scalefactors::{self, Scalefactors};
use crate::sideinfo::{SHORT_BLOCK, SideInfo};
use crate::stereo::{self, Pan};
use crate::synth::{self, HISTORY, ROWS};
use crate::{Error, FrameHeader, huffman, hybrid, requantize};
use std::sync::OnceLock;

/// The bit reservoir: main data can start this many bytes before its frame (MPEG-1's 9-bit
/// `main_data_begin`; MPEG-2's 8 bits reach 255).
const RESERVOIR: usize = 511;
/// The longest frame decoded: MPEG-1 Layer III free format at 640 kbit/s and 32 kHz is 2881
/// bytes. Longer frames are refused, so the decoder's buffers never grow.
pub const MAX_FRAME_LEN: usize = 4096;

/// What a decoded frame was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub header: FrameHeader,
    /// Sample frames appended (per channel): the header's samples per frame.
    pub samples: usize,
}

pub(crate) struct Tables {
    /// `i^(4/3)` for every value a Huffman code with linbits can give (15 + 8191).
    pow43: Vec<f32>,
    /// `2^(e/4)` from `e = GAIN_MIN`.
    gains: Vec<f32>,
    pan: Pan,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables {
        pow43: (0..8207)
            .map(|i| libm::pow(f64::from(i), 4.0 / 3.0) as f32)
            .collect(),
        gains: (0..576)
            .map(|i| libm::exp2(f64::from(i + GAIN_MIN) / 4.0) as f32)
            .collect(),
        pan: Pan::new(),
    })
}

/// Decodes Layer III frames, one whole frame at a time, into interleaved `f32` samples.
///
/// Output is nominally within -1..1 and always finite and within
/// [`OUTPUT_LIMIT`](crate::OUTPUT_LIMIT). The decoder delays its output by 529 sample frames
/// (the hybrid filterbank's 528 and one of the synthesis's alignment): what an encoder put at
/// sample `n` comes out at `n + 529`.
pub struct Decoder {
    tables: &'static Tables,
    /// The last main-data bytes of the frames seen, oldest first (at most `RESERVOIR`).
    reservoir: Vec<u8>,
    /// This frame's main data: the reservoir's tail, then the frame's own.
    main: Vec<u8>,
    overlap: [[f32; 288]; 2],
    history: [f32; HISTORY],
    xr: Box<[[f32; 576]; 2]>,
    scratch: Box<[f32; 576]>,
    lins: Box<[[f32; 4]; ROWS]>,
    /// MPEG-1 granule 0's scalefactors per channel, which granule 1 may reuse.
    granule0: [Scalefactors; 2],
    /// The channel count of the last frame decoded.
    channels: u8,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            tables: tables(),
            reservoir: Vec::with_capacity(RESERVOIR + MAX_FRAME_LEN),
            main: Vec::with_capacity(RESERVOIR + MAX_FRAME_LEN),
            overlap: [[0.0; 288]; 2],
            history: [0.0; HISTORY],
            xr: Box::new([[0.0; 576]; 2]),
            scratch: Box::new([0.0; 576]),
            lins: Box::new([[0.0; 4]; ROWS]),
            granule0: [Scalefactors::ZERO; 2],
            channels: 0,
        }
    }

    /// Forgets the reservoir, the IMDCT overlap and the filterbank history, as if new.
    pub fn reset(&mut self) {
        self.reservoir.clear();
        self.overlap = [[0.0; 288]; 2];
        self.history = [0.0; HISTORY];
        self.granule0 = [Scalefactors::ZERO; 2];
    }

    /// Decodes one whole frame (header through its last byte) and appends its samples to
    /// `out`, interleaved: always exactly samples-per-frame times channels of them once the
    /// first four bytes are a frame header, silence when the frame cannot be decoded.
    ///
    /// The errors that still append silence: [`Error::Reservoir`] (normal for the first frames
    /// after the start or a [`reset`](Self::reset); see [`Error::is_reservoir`]),
    /// [`Error::UnsupportedLayer`], [`Error::FrameLength`], [`Error::Crc`] and
    /// [`Error::Corrupt`]. Only [`Error::NotAHeader`] appends nothing. A frame that fails still
    /// leaves its main data in the reservoir for the frames after it.
    ///
    /// A frame with a CRC is checked, and a mismatch is an error: the CRC protects the side
    /// information, and decoding damaged side information gives loud garbage.
    pub fn decode_frame(&mut self, frame: &[u8], out: &mut Vec<f32>) -> Result<FrameInfo, Error> {
        let Some(head) = frame.first_chunk::<4>() else {
            return Err(Error::NotAHeader("shorter than a header"));
        };
        let header = FrameHeader::parse(head)?;
        let samples = header.samples_per_frame() as usize;
        let start = out.len();
        out.resize(start + samples * usize::from(header.channels()), 0.0);
        let result = self.decode_into(&header, frame, &mut out[start..]);
        if result.is_err() {
            out[start..].fill(0.0);
        }
        result.map(|()| FrameInfo { header, samples })
    }

    fn decode_into(&mut self, h: &FrameHeader, frame: &[u8], pcm: &mut [f32]) -> Result<(), Error> {
        if h.layer != Layer::III {
            return Err(Error::UnsupportedLayer(h.layer));
        }
        let side_start = h.data_start();
        let main_start = side_start + h.side_info_len();
        let expected = h.frame_len().unwrap_or(main_start);
        let fits = match h.frame_len() {
            Some(len) => frame.len() == len,
            None => frame.len() >= main_start,
        };
        if !fits || frame.len() > MAX_FRAME_LEN {
            return Err(Error::FrameLength {
                expected,
                actual: frame.len(),
            });
        }
        let region = &frame[main_start..];
        let side = SideInfo::parse(h, &frame[side_start..main_start]);
        let crc = if h.crc {
            check_crc(frame, main_start)
        } else {
            Ok(())
        };
        let ready = match (&side, crc) {
            (_, Err(e)) => Err(e),
            (Err(e), _) => Err(e.clone()),
            (Ok(side), Ok(())) => self.assemble(side.main_data_begin, region),
        };
        self.keep_reservoir(region);
        ready?;
        let side = side?;
        self.decode_granules(h, &side, pcm)
    }

    /// Puts this frame's main data together in `main`.
    fn assemble(&mut self, begin: usize, region: &[u8]) -> Result<(), Error> {
        let held = self.reservoir.len();
        if begin > held {
            return Err(Error::Reservoir {
                needed: begin,
                available: held,
            });
        }
        self.main.clear();
        self.main.extend_from_slice(&self.reservoir[held - begin..]);
        self.main.extend_from_slice(region);
        Ok(())
    }

    fn keep_reservoir(&mut self, region: &[u8]) {
        self.reservoir.extend_from_slice(region);
        let over = self.reservoir.len().saturating_sub(RESERVOIR);
        self.reservoir.drain(..over);
    }

    fn decode_granules(
        &mut self,
        h: &FrameHeader,
        side: &SideInfo,
        pcm: &mut [f32],
    ) -> Result<(), Error> {
        let nch = usize::from(h.channels());
        if h.channels() != self.channels {
            // A switch between mono and stereo starts the filterbank afresh, as the ISO
            // conformance stream l3_10201's references (one per segment) require.
            self.overlap = [[0.0; 288]; 2];
            self.history = [0.0; HISTORY];
        }
        self.channels = h.channels();
        let lsf = h.version.is_lsf();
        let total: usize = side.granules[..side.n_granules]
            .iter()
            .flat_map(|g| g[..nch].iter())
            .map(|g| usize::from(g.part2_3_length))
            .sum();
        if total > self.main.len() * 8 {
            return Err(Error::Corrupt("granules longer than the main data"));
        }
        let t = self.tables;
        let joint = h.mode == ChannelMode::JointStereo && nch == 2;
        let mut pos = 0;
        for (gr, granules) in side.granules[..side.n_granules].iter().enumerate() {
            let mut sf = [Scalefactors::ZERO; 2];
            for ch in 0..nch {
                let g = &granules[ch];
                let end = pos + usize::from(g.part2_3_length);
                let mut b = Bits::at(&self.main, pos);
                sf[ch] = if lsf {
                    scalefactors::read_lsf(&mut b, g, h.intensity_stereo() && ch == 1)
                } else {
                    scalefactors::read_mpeg1(&mut b, g, gr, &self.granule0[ch])
                };
                if !lsf && gr == 0 {
                    self.granule0[ch] = sf[ch];
                }
                let xr = &mut self.xr[ch];
                let n = huffman::decode(&mut b, g, end, &t.pow43, xr);
                requantize::apply(xr, g, &sf[ch], n, &t.gains);
                pos = end;
            }
            if joint {
                let [left, right] = &mut *self.xr;
                if h.intensity_stereo() {
                    stereo::intensity(
                        left,
                        right,
                        &granules[1],
                        &sf[1],
                        lsf,
                        h.ms_stereo(),
                        &t.pan,
                    );
                } else if h.ms_stereo() {
                    stereo::mid_side(left, right);
                }
            }
            for (ch, g) in granules.iter().enumerate().take(nch) {
                let xr = &mut self.xr[ch];
                let long_subbands = usize::from(g.bands.long_subbands);
                let boundaries = if g.block_type == SHORT_BLOCK {
                    hybrid::reorder(&mut xr[18 * long_subbands..], &mut self.scratch, &g.bands);
                    long_subbands.saturating_sub(1)
                } else {
                    31
                };
                hybrid::antialias(xr, boundaries);
                hybrid::imdct(xr, &mut self.overlap[ch], g.block_type, long_subbands);
                hybrid::invert_odd(xr);
            }
            let out = &mut pcm[gr * 576 * nch..(gr + 1) * 576 * nch];
            synth::granule(&mut self.history, &mut self.xr, nch, out, &mut self.lins);
        }
        Ok(())
    }
}

/// The CRC-16 (polynomial 0x8005, initial 0xFFFF) over the header's last two bytes and the
/// side information, against the two bytes after the header.
fn check_crc(frame: &[u8], main_start: usize) -> Result<(), Error> {
    let expected = u16::from_be_bytes([frame[4], frame[5]]);
    let mut crc: u16 = 0xFFFF;
    for &byte in frame[2..4].iter().chain(&frame[6..main_start]) {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    if crc == expected {
        Ok(())
    } else {
        Err(Error::Crc {
            expected,
            actual: crc,
        })
    }
}
