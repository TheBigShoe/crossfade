//! Channel mixing to a new layout: the standard down- and up-mixes, or a matrix.
//!
//! The filter is a stateless per-frame matrix, `out[o] = sum_i m[o][i] * in[i]`, summed in
//! input order and skipping zero coefficients (so a silent output is `+0.0`, and a `-0.0`
//! or non-finite input never reaches an output it has no gain to). A matrix you give is used
//! as given.
//!
//! The standard mix is chosen by speaker position, never channel index:
//! - the same positions, in any order: a bit-exact copy or permutation;
//! - mono to stereo: the signal to both sides at 1.0 (a mono-stereo-mono round trip is unity);
//! - down to stereo: ITU-R BS.775, `L = FL + 0.7071 FC + 0.7071 Ls`, likewise `R`. Side and
//!   back surrounds are both "surround" (with both present each pair gets 0.7071), and a back
//!   centre feeds both sides at 0.7071. The LFE is dropped, as BS.775 has no place for it;
//! - down to mono: that stereo mix, then `0.5 (L + R)`, composed into one matrix;
//! - up-mixes (including stereo or mono to 5.1) invent nothing: each input goes to the same
//!   position in the output, every other output channel is silent;
//! - unpositioned channels are relabelled when the counts match.
//!
//! Level convention: a downmix matrix is divided by its largest row sum of absolute
//! coefficients, so full scale on every input at once is full scale out and cannot clip.
//! Anything else is refused, naming both layouts and suggesting `matrix=`.

use core::f64::consts::FRAC_1_SQRT_2 as R;
use crossfade_core::{AudioFrame, AudioSpec, Channel, ChannelLayout, Error, Filter, Samples};

#[derive(Debug, Clone, PartialEq)]
pub struct MixPlan {
    pub layout: ChannelLayout,
    /// `matrix[out][in]`, linear gains; `None` picks the standard mix for the two layouts.
    pub matrix: Option<Vec<Vec<f64>>>,
}

pub struct Mix {
    in_channels: usize,
    out_channels: usize,
    out_spec: AudioSpec,
    /// Per output channel, its nonzero `(input, gain)` terms in input order.
    rows: Vec<Vec<(usize, f64)>>,
    buf: Vec<f64>,
    pts: i64,
    ready: bool,
}

impl Mix {
    pub fn new(input: &AudioSpec, plan: &MixPlan) -> Result<Self, Error> {
        let matrix = match &plan.matrix {
            Some(m) => {
                super::check_matrix(m, input.layout().channels(), &plan.layout)?;
                m.clone()
            }
            None => standard_matrix(input.layout(), &plan.layout)?,
        };
        let rows = matrix
            .iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .filter(|(_, c)| **c != 0.0)
                    .map(|(i, c)| (i, *c))
                    .collect()
            })
            .collect();
        Ok(Self {
            in_channels: usize::from(input.layout().channels()),
            out_channels: usize::from(plan.layout.channels()),
            out_spec: AudioSpec::new(input.sample_rate(), input.format(), plan.layout.clone())?,
            rows,
            buf: Vec::new(),
            pts: 0,
            ready: false,
        })
    }
}

/// The standard mix from `from` to `to`, `m[out][in]`; see the module doc for the rules.
pub fn standard_matrix(from: &ChannelLayout, to: &ChannelLayout) -> Result<Vec<Vec<f64>>, Error> {
    let (n_in, n_out) = (usize::from(from.channels()), usize::from(to.channels()));
    let refuse = || Error::Unsupported {
        what: format!(
            "no standard mix from {} to {}; give one with mix ... matrix=...",
            super::layout_words(from),
            super::layout_words(to)
        ),
    };
    let mut m = vec![vec![0.0; n_in]; n_out];
    let (Some(f), Some(t)) = (from.positions(), to.positions()) else {
        if n_in != n_out {
            return Err(refuse());
        }
        for (o, row) in m.iter_mut().enumerate() {
            row[o] = 1.0;
        }
        return Ok(m);
    };
    let has = |set: &[Channel], c: Channel| set.contains(&c);
    let into_same_positions = |m: &mut Vec<Vec<f64>>| {
        for (i, c) in f.iter().enumerate() {
            let o = t.iter().position(|x| x == c).expect("checked subset");
            m[o][i] = 1.0;
        }
    };
    if f.iter().all(|c| has(t, *c)) {
        into_same_positions(&mut m);
        return Ok(m);
    }
    if f == [Channel::FrontCenter]
        && t.len() == 2
        && has(t, Channel::FrontLeft)
        && has(t, Channel::FrontRight)
    {
        m.iter_mut().for_each(|row| row[0] = 1.0);
        return Ok(m);
    }
    let to_stereo = t.len() == 2 && has(t, Channel::FrontLeft) && has(t, Channel::FrontRight);
    if !(to_stereo || t == [Channel::FrontCenter]) {
        return Err(refuse());
    }
    // (to left, to right) for each input position.
    let pair = |c: Channel| -> Option<(f64, f64)> {
        use Channel::*;
        Some(match c {
            FrontLeft => (1.0, 0.0),
            FrontRight => (0.0, 1.0),
            FrontCenter | BackCenter => (R, R),
            Lfe => (0.0, 0.0),
            BackLeft | SideLeft => (R, 0.0),
            BackRight | SideRight => (0.0, R),
            _ => return None,
        })
    };
    let pairs: Option<Vec<(f64, f64)>> = f.iter().map(|c| pair(*c)).collect();
    let pairs = pairs.ok_or_else(refuse)?;
    for (o, row) in m.iter_mut().enumerate() {
        for (i, (l, r)) in pairs.iter().enumerate() {
            row[i] = if to_stereo {
                if t[o] == Channel::FrontLeft { *l } else { *r }
            } else {
                0.5 * (l + r)
            };
        }
    }
    let peak = m
        .iter()
        .map(|row| row.iter().map(|c| c.abs()).sum::<f64>())
        .fold(0.0, f64::max);
    if peak > 1.0 {
        m.iter_mut().flatten().for_each(|c| *c /= peak);
        m.iter_mut().for_each(|row| settle_to_unity(row));
    }
    Ok(m)
}

/// Division rounds each coefficient, so a row can add up to one ulp past 1.0 and clip a
/// full-scale input. Nudge the largest coefficient until the sum, taken in the filter's own
/// order, is exactly 1.0 (a handful of ulps at most).
fn settle_to_unity(row: &mut [f64]) {
    let sum = |row: &[f64]| row.iter().filter(|c| **c != 0.0).fold(0.0, |a, c| a + c);
    let Some(big) = (0..row.len()).max_by(|&a, &b| row[a].total_cmp(&row[b])) else {
        return;
    };
    if (sum(row) - 1.0).abs() > 1e-9 {
        return;
    }
    for _ in 0..8 {
        let s = sum(row);
        if s == 1.0 {
            break;
        }
        let bits = row[big].to_bits();
        row[big] = f64::from_bits(if s > 1.0 { bits - 1 } else { bits + 1 });
    }
}

impl Filter for Mix {
    fn output_spec(&self) -> &AudioSpec {
        &self.out_spec
    }

    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F64(s) = &frame.samples else {
            return Err(super::not_working_format("mix", frame));
        };
        if s.len() % self.in_channels != 0 {
            return Err(Error::Malformed {
                what: format!(
                    "{} samples is not a whole number of {}-channel frames",
                    s.len(),
                    self.in_channels
                ),
            });
        }
        self.buf.clear();
        for f in s.chunks_exact(self.in_channels) {
            for row in &self.rows {
                let mut terms = row.iter();
                let first = terms.next().map_or(0.0, |&(i, c)| c * f[i]);
                self.buf
                    .push(terms.fold(first, |acc, &(i, c)| acc + c * f[i]));
            }
        }
        debug_assert_eq!(self.buf.len() % self.out_channels, 0);
        self.pts = frame.pts;
        self.ready = !self.buf.is_empty();
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if !std::mem::take(&mut self.ready) {
            return Ok(false);
        }
        out.sample_rate = self.out_spec.sample_rate();
        out.layout.clone_from(self.out_spec.layout());
        out.pts = self.pts;
        // Swap buffers so neither side allocates in steady state.
        let mut spare = match std::mem::replace(&mut out.samples, Samples::F64(Vec::new())) {
            Samples::F64(v) => v,
            _ => Vec::new(),
        };
        std::mem::swap(&mut spare, &mut self.buf);
        out.samples = Samples::F64(spare);
        Ok(true)
    }

    /// Holds nothing back.
    fn drain(&mut self) {}
}
