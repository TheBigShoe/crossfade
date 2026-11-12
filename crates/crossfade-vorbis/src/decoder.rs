//! The audio packet decode (specification section 4.3): floors, residues, coupling, the
//! IMDCT, and the overlap-add from the centre of the previous window to the centre of this
//! one.
//!
//! Windows overlap by the actual sizes of the two blocks, as libvorbis does (the window flags
//! in a long packet are read and not trusted), so a stream whose flags disagree with its
//! blocks still decodes. Each channel keeps the DCT-IV of its last block, from which both
//! halves of the IMDCT are read directly.

use crate::bits::BitReader;
use crate::error::{Error, Result, malformed};
use crate::floor::Floor;
use crate::header::Ident;
use crate::mdct::{self, Imdct};
use crate::residue;
use crate::setup::Setup;

/// Every sample out lies within this, whatever the stream holds (valid streams stay near 1).
pub(crate) const OUTPUT_LIMIT: f32 = 16.0;

/// The most floor 1 posts and floor 0 coefficients a channel can need.
const MAX_POSTS: usize = 65;
const MAX_ORDER: usize = 256;

/// Decodes the audio packets of one stream into interleaved `f32` samples.
#[derive(Debug, Clone)]
pub struct Decoder {
    setup: Setup,
    channels: usize,
    sizes: [usize; 2],
    /// Half the long block: the stride of every per-channel buffer.
    stride: usize,
    imdct: [Imdct; 2],
    /// The window slope for an overlap of each block size's half.
    windows: [Vec<f32>; 2],
    fft: mdct::Scratch,
    residue: residue::Scratch,
    /// This block's spectra, then their DCT-IV.
    cur: Vec<f32>,
    /// The previous block's DCT-IV.
    prev: Vec<f32>,
    /// The previous block's size; 0 before the first packet.
    prev_n: usize,
    pcm: Vec<f32>,
    used: Vec<bool>,
    coded: Vec<bool>,
    floor_y: Vec<i32>,
    floor_coeffs: Vec<f32>,
    floor_amp: Vec<u64>,
    /// Floor 0's work space: two values per line at most.
    floor_scratch: Vec<f32>,
    /// The channels of each submap of each mapping.
    submap_channels: Vec<Vec<Vec<usize>>>,
    /// Where each channel goes in an output frame.
    order: Vec<usize>,
    #[cfg(feature = "profile")]
    times: [std::time::Duration; STAGES.len()],
}

/// The stages [`Decoder::stage_times`] reports.
#[cfg(feature = "profile")]
pub const STAGES: [&str; 6] = [
    "floor decode",
    "residue decode",
    "coupling and floor curve",
    "IMDCT",
    "overlap-add",
    "interleave",
];

impl Decoder {
    /// A decoder for the stream these headers describe. The identification header must be
    /// the one the setup was parsed with.
    pub fn new(ident: &Ident, setup: Setup) -> Result<Self> {
        if setup.ident != *ident {
            return Err(malformed(
                "the setup header was parsed for another identification header",
            ));
        }
        let channels = usize::from(ident.channels);
        let sizes = ident.block_sizes.map(usize::from);
        let stride = sizes[1] / 2;
        let window = |n: usize| {
            let s = n / 2;
            (0..s)
                .map(|k| {
                    let x = libm::sin((k as f64 + 0.5) / s as f64 * std::f64::consts::FRAC_PI_2);
                    libm::sin(std::f64::consts::FRAC_PI_2 * x * x) as f32
                })
                .collect::<Vec<f32>>()
        };
        let submap_channels = setup
            .mappings
            .iter()
            .map(|m| {
                (0..m.submaps.len())
                    .map(|s| {
                        (0..channels)
                            .filter(|&c| usize::from(m.mux[c]) == s)
                            .collect()
                    })
                    .collect()
            })
            .collect();
        Ok(Self {
            channels,
            sizes,
            stride,
            imdct: [Imdct::new(sizes[0]), Imdct::new(sizes[1])],
            windows: [window(sizes[0]), window(sizes[1])],
            fft: mdct::Scratch::new(sizes[1]),
            residue: residue::Scratch::new(channels, stride),
            cur: vec![0.0; channels * stride],
            prev: vec![0.0; channels * stride],
            prev_n: 0,
            pcm: vec![0.0; channels * stride],
            used: vec![false; channels],
            coded: vec![false; channels],
            floor_y: vec![0; channels * MAX_POSTS],
            floor_coeffs: vec![0.0; channels * MAX_ORDER],
            floor_amp: vec![0; channels],
            floor_scratch: vec![0.0; 2 * stride],
            submap_channels,
            order: (0..channels).collect(),
            setup,
            #[cfg(feature = "profile")]
            times: Default::default(),
        })
    }

    /// Roughly the bytes a decoder for this setup holds besides the setup itself, for a
    /// caller that checks it against a limit before calling [`Decoder::new`]. Bounded by the
    /// identification header: at most a few megabytes.
    pub fn bytes_needed(setup: &Setup) -> usize {
        let channels = usize::from(setup.ident.channels);
        let sizes = setup.ident.block_sizes.map(usize::from);
        let stride = sizes[1] / 2;
        channels * stride * 4 * 3
            + residue::Scratch::bytes(channels, stride)
            + mdct::Scratch::bytes(sizes[1])
            + Imdct::bytes(sizes[0])
            + Imdct::bytes(sizes[1])
            + (sizes[0] + sizes[1]) * 2
            + channels * (MAX_POSTS * 4 + MAX_ORDER * 4 + 8 + 2 + 8 * 2)
            + 2 * stride * 4
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The setup this decoder was made with.
    pub fn setup(&self) -> &Setup {
        &self.setup
    }

    /// Puts channel `c` (in Vorbis order) at position `order[c]` of each output frame. False,
    /// and nothing changed, unless `order` is a permutation of the channels.
    pub fn set_channel_order(&mut self, order: &[usize]) -> bool {
        let mut seen = vec![false; self.channels];
        if order.len() != self.channels {
            return false;
        }
        for &o in order {
            if o >= self.channels || seen[o] {
                return false;
            }
            seen[o] = true;
        }
        self.order.copy_from_slice(order);
        true
    }

    /// Time spent in each of [`STAGES`] so far.
    #[cfg(feature = "profile")]
    pub fn stage_times(&self) -> [std::time::Duration; STAGES.len()] {
        self.times
    }

    /// Forgets the overlap: the next packet yields nothing, as the first one does.
    pub fn reset(&mut self) {
        self.prev_n = 0;
    }

    /// Decodes one audio packet, appending its frames (interleaved, nominally -1 to 1) to
    /// `out`, and returns how many frames it appended. A packet that cannot be decoded (a
    /// header, an empty packet, a mode that does not exist, one cut off before its window
    /// flags) is an error that changes nothing; a packet that ends early otherwise decodes,
    /// its spectrum ending where the data does.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<usize> {
        #[cfg(feature = "profile")]
        let mut clock = std::time::Instant::now();
        macro_rules! lap {
            ($stage:expr) => {
                #[cfg(feature = "profile")]
                #[allow(unused_assignments)]
                {
                    let now = std::time::Instant::now();
                    self.times[$stage] += now - clock;
                    clock = now;
                }
            };
        }
        let setup = &self.setup;
        let mut r = BitReader::new(packet);
        let mode = setup.modes().read_mode(&mut r)?;
        let long = setup.modes().is_long(mode);
        if long && r.read(2).is_none() {
            return Err(Error::BadPacket("packet ends before its window flags"));
        }
        let n = self.sizes[usize::from(long)];
        let half = n / 2;
        let stride = self.stride;
        let mapping_index = usize::from(setup.mode_mappings[mode]);
        let mapping = &setup.mappings[mapping_index];
        let books = &setup.codebooks;

        for ch in 0..self.channels {
            let submap = usize::from(mapping.mux[ch]);
            let floor = &setup.floors[usize::from(mapping.submaps[submap].0)];
            self.used[ch] = match floor {
                Floor::Zero(f) => {
                    let coeffs = &mut self.floor_coeffs[ch * MAX_ORDER..(ch + 1) * MAX_ORDER];
                    match f.decode(books, &mut r, coeffs) {
                        Some(amp) => {
                            self.floor_amp[ch] = amp;
                            true
                        }
                        None => false,
                    }
                }
                Floor::One(f) => {
                    let ys = &mut self.floor_y[ch * MAX_POSTS..(ch + 1) * MAX_POSTS];
                    f.decode(books, &mut r, ys)
                }
            };
        }

        lap!(0);
        self.coded.copy_from_slice(&self.used);
        for &(m, a) in &mapping.coupling {
            let (m, a) = (usize::from(m), usize::from(a));
            if self.coded[m] || self.coded[a] {
                self.coded[m] = true;
                self.coded[a] = true;
            }
        }

        for ch in 0..self.channels {
            self.cur[ch * stride..ch * stride + half].fill(0.0);
        }
        for (s, &(_, res)) in mapping.submaps.iter().enumerate() {
            setup.residues[usize::from(res)].decode(
                books,
                &mut r,
                &self.submap_channels[mapping_index][s],
                &self.coded,
                &mut self.cur,
                stride,
                half,
                &mut self.residue,
            );
        }

        lap!(1);
        for &(m, a) in mapping.coupling.iter().rev() {
            let (m, a) = (usize::from(m), usize::from(a));
            let (mag, ang) = if m < a {
                let (lo, hi) = self.cur.split_at_mut(a * stride);
                (&mut lo[m * stride..m * stride + half], &mut hi[..half])
            } else {
                let (lo, hi) = self.cur.split_at_mut(m * stride);
                (&mut hi[..half], &mut lo[a * stride..a * stride + half])
            };
            uncouple(mag, ang);
        }

        for ch in 0..self.channels {
            let spectrum = &mut self.cur[ch * stride..ch * stride + half];
            if !self.used[ch] {
                spectrum.fill(0.0);
                continue;
            }
            let submap = usize::from(mapping.mux[ch]);
            match &setup.floors[usize::from(mapping.submaps[submap].0)] {
                Floor::Zero(f) => f.apply(
                    long,
                    self.floor_amp[ch],
                    &self.floor_coeffs[ch * MAX_ORDER..(ch + 1) * MAX_ORDER],
                    spectrum,
                    &mut self.floor_scratch,
                ),
                Floor::One(f) => f.apply(&self.floor_y[ch * MAX_POSTS..], spectrum),
            }
            lap!(2);
            self.imdct[usize::from(long)].dct4(spectrum, &mut self.fft);
            lap!(3);
        }
        lap!(2);

        let frames = if self.prev_n == 0 {
            0
        } else {
            self.prev_n / 4 + n / 4
        };
        if frames > 0 {
            let window = &self.windows[usize::from(self.prev_n.min(n) == self.sizes[1])];
            for ch in 0..self.channels {
                overlap(
                    &self.prev[ch * stride..ch * stride + self.prev_n / 2],
                    &self.cur[ch * stride..ch * stride + half],
                    window,
                    &mut self.pcm[ch * stride..ch * stride + frames],
                );
            }
        }
        lap!(4);
        std::mem::swap(&mut self.cur, &mut self.prev);
        self.prev_n = n;
        self.interleave(frames, out);
        lap!(5);
        Ok(frames)
    }

    fn interleave(&self, frames: usize, out: &mut Vec<f32>) {
        if frames == 0 {
            return;
        }
        // Not f32::clamp, which passes NaN through: max and min drop it.
        #[allow(clippy::manual_clamp)]
        let clamp = |v: f32| v.max(-OUTPUT_LIMIT).min(OUTPUT_LIMIT);
        let stride = self.stride;
        let start = out.len();
        match self.channels {
            1 => out.extend(self.pcm[..frames].iter().map(|&v| clamp(v))),
            2 => {
                let (a, b) = (&self.pcm[..frames], &self.pcm[stride..stride + frames]);
                let (a, b) = if self.order[0] == 0 { (a, b) } else { (b, a) };
                out.extend(a.iter().zip(b).flat_map(|(&x, &y)| [clamp(x), clamp(y)]));
            }
            c => {
                out.resize(start + frames * c, 0.0);
                let dst = &mut out[start..];
                for ch in 0..c {
                    let src = &self.pcm[ch * stride..ch * stride + frames];
                    for (d, &s) in dst[self.order[ch]..].iter_mut().step_by(c).zip(src) {
                        *d = clamp(s);
                    }
                }
            }
        }
    }
}

/// Square polar mapping undone (section 4.3.5), in place.
fn uncouple(mag: &mut [f32], ang: &mut [f32]) {
    for (m, a) in mag.iter_mut().zip(ang.iter_mut()) {
        let (mv, av) = (*m, *a);
        let (nm, na) = if mv > 0.0 {
            if av > 0.0 {
                (mv, mv - av)
            } else {
                (mv + av, mv)
            }
        } else if av > 0.0 {
            (mv, mv + av)
        } else {
            (mv - av, mv)
        };
        *m = nm;
        *a = na;
    }
}

/// The output between the previous window's centre and this one's, from the DCT-IV of both
/// blocks: `zp` (the previous, `P/2` long) and `zc` (this one, `C/2`), overlapping over the
/// slope `w` (`min(P, C)/2` long). `out` is `P/4 + C/4` long. With `M` lines, the IMDCT is
/// `z[t + M/2]` for `t < M/2`, `-z[3M/2 - 1 - t]` up to `3M/2`, `-z[t - 3M/2]` after.
fn overlap(zp: &[f32], zc: &[f32], w: &[f32], out: &mut [f32]) {
    let (mp, mc, s) = (zp.len(), zc.len(), w.len());
    let (hp, s2) = (mp / 2, s / 2);
    // Before the slope: the previous block alone.
    for (o, &z) in out[..hp - s2].iter_mut().zip(zp[s2..hp].iter().rev()) {
        *o = -z;
    }
    // The first half of the slope.
    let (w_lo, w_hi) = w.split_at(s2);
    for (((o, &p), &c), (&wa, &wd)) in out[hp - s2..hp]
        .iter_mut()
        .zip(zp[..s2].iter().rev())
        .zip(&zc[mc - s2..])
        .zip(w_lo.iter().zip(w_hi.iter().rev()))
    {
        *o = c * wa - p * wd;
    }
    // The second half.
    for (((o, &p), &c), (&wd, &wa)) in out[hp..hp + s2]
        .iter_mut()
        .zip(&zp[..s2])
        .zip(zc[..mc].iter().rev())
        .zip(w_lo.iter().rev().zip(w_hi))
    {
        *o = -(p * wd) - c * wa;
    }
    // After the slope: this block alone.
    let rest = out.len() - hp - s2;
    for (o, &z) in out[hp + s2..]
        .iter_mut()
        .zip(zc[..mc - s2].iter().rev().take(rest))
    {
        *o = -z;
    }
}

#[cfg(test)]
#[path = "decoder_tests.rs"]
mod tests;
