//! The streaming meter.
//!
//! Samples are processed one at a time, so the result cannot depend on how the input is cut
//! into `push` calls. Energy is summed per 20 ms sub-block; every window (400 ms momentary,
//! 3 s short-term) is a fixed-order sum of whole sub-blocks, so the 100 ms hop of the gating
//! blocks and the finer momentary and short-term readings need no running totals to drift.
//!
//! Only `+ - * /`, `sqrt`, comparisons and `libm` are used: no `std` transcendental function
//! and no `mul_add`, so the bits are the same on every platform.

use crate::MAX_CHANNELS;
use crate::channel::ChannelRole;
use crate::error::{Error, Result};
use crate::kweight::{Biquad, KWeighting};
use crate::peak::{History, Interpolator, factor_for};

/// Sub-blocks per second (20 ms each).
const SUB_PER_SEC: u64 = 50;
const MOMENTARY_SUBS: usize = 20;
const SHORT_TERM_SUBS: usize = 150;
/// Gating blocks start every 100 ms: every fifth sub-block.
const GATE_HOP_SUBS: u64 = 5;
const ABS_GATE_LUFS: f64 = -70.0;
const OFFSET: f64 = -0.691;
/// Larger magnitudes are clamped: their squares could overflow a block sum.
const MAX_ABS_SAMPLE: f64 = 1.0e12;

/// Which of the optional, costlier measurements to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// True peak (oversampled); sample peak is always measured.
    pub true_peak: bool,
    /// Loudness range (EBU Tech 3342).
    pub loudness_range: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            true_peak: true,
            loudness_range: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Stage {
    s1: f64,
    s2: f64,
}

impl Stage {
    #[inline]
    fn run(&mut self, c: &Biquad, x: f64) -> f64 {
        let y = c.b[0] * x + self.s1;
        self.s1 = c.b[1] * x - c.a[1] * y + self.s2;
        self.s2 = c.b[2] * x - c.a[2] * y;
        y
    }
}

#[derive(Debug, Clone)]
pub struct Meter {
    rate: u32,
    options: Options,
    roles: Vec<ChannelRole>,
    weights: Vec<f64>,
    k: KWeighting,
    abs_gate_power: f64,

    stages: Vec<[Stage; 2]>,
    sum_sq: Vec<f64>,
    cursor: usize,
    frames: u64,
    last_boundary: u64,
    next_boundary: u64,
    sub_count: u64,
    ring_energy: [f64; SHORT_TERM_SUBS],
    ring_len: [u64; SHORT_TERM_SUBS],

    momentary: f64,
    momentary_max: f64,
    short_term: f64,
    short_term_max: f64,
    /// Powers of the 400 ms blocks above the absolute gate, one per 100 ms.
    gate_blocks: Vec<f64>,
    /// Powers of the 3 s blocks at or above the absolute gate, one per 100 ms (range only).
    range_blocks: Vec<f64>,
    sanitised: u64,

    sample_peaks: Vec<f64>,
    interp: Interpolator,
    /// Per-channel history and running true peak; empty with true peak off.
    true_peaks: Vec<(History, f64)>,
}

impl Meter {
    /// A meter for interleaved input with one entry of `roles` per channel.
    pub fn new(sample_rate: u32, roles: &[ChannelRole], options: Options) -> Result<Self> {
        let k = KWeighting::new(sample_rate)?;
        if roles.is_empty() {
            return Err(Error::NoChannels);
        }
        if roles.len() > MAX_CHANNELS {
            return Err(Error::TooManyChannels { count: roles.len() });
        }
        let n = roles.len();
        Ok(Self {
            rate: sample_rate,
            options,
            roles: roles.to_vec(),
            weights: roles.iter().map(|r| r.weight()).collect(),
            k,
            abs_gate_power: libm::pow(10.0, (ABS_GATE_LUFS - OFFSET) / 10.0),
            stages: vec![[Stage::default(); 2]; n],
            sum_sq: vec![0.0; n],
            cursor: 0,
            frames: 0,
            last_boundary: 0,
            next_boundary: u64::from(sample_rate) / SUB_PER_SEC,
            sub_count: 0,
            ring_energy: [0.0; SHORT_TERM_SUBS],
            ring_len: [0; SHORT_TERM_SUBS],
            momentary: 0.0,
            momentary_max: 0.0,
            short_term: 0.0,
            short_term_max: 0.0,
            gate_blocks: Vec::new(),
            range_blocks: Vec::new(),
            sanitised: 0,
            sample_peaks: vec![0.0; n],
            interp: Interpolator::new(factor_for(sample_rate)),
            true_peaks: if options.true_peak {
                vec![(History::new(), 0.0); n]
            } else {
                Vec::new()
            },
        })
    }

    /// Forget everything measured; the configuration stays.
    pub fn reset(&mut self) {
        let fresh = Self::new(self.rate, &self.roles, self.options).expect("config was valid");
        *self = fresh;
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn channels(&self) -> usize {
        self.roles.len()
    }

    /// Whole frames pushed so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// How many samples were not finite, or beyond +-1e12, and so replaced (see the crate doc).
    pub fn sanitised_samples(&self) -> u64 {
        self.sanitised
    }

    /// Feed interleaved samples, any number, even half a frame: the rest of the frame can
    /// come with the next call.
    pub fn push(&mut self, samples: &[f64]) {
        let channels = self.roles.len();
        for &raw in samples {
            let c = self.cursor;
            let x = self.sanitise(raw);
            if self.roles[c] != ChannelRole::Excluded {
                self.sample_peaks[c] = self.sample_peaks[c].max(x.abs());
                if let Some((hist, peak)) = self.true_peaks.get_mut(c) {
                    *peak = peak.max(hist.push(&self.interp, x));
                }
            }
            if self.weights[c] != 0.0 {
                let st = &mut self.stages[c];
                let shelved = st[0].run(&self.k.shelf, x);
                let y = st[1].run(&self.k.highpass, shelved);
                self.sum_sq[c] += y * y;
            }
            self.cursor += 1;
            if self.cursor == channels {
                self.cursor = 0;
                self.frames += 1;
                if self.frames == self.next_boundary {
                    self.close_sub_block();
                }
            }
        }
    }

    #[inline]
    fn sanitise(&mut self, x: f64) -> f64 {
        if !x.is_finite() {
            self.sanitised += 1;
            0.0
        } else if x.abs() > MAX_ABS_SAMPLE {
            self.sanitised += 1;
            MAX_ABS_SAMPLE.copysign(x)
        } else {
            x
        }
    }

    fn close_sub_block(&mut self) {
        let mut energy = 0.0;
        for (sum, w) in self.sum_sq.iter_mut().zip(&self.weights) {
            if *w != 0.0 {
                energy += w * *sum;
            }
            *sum = 0.0;
        }
        let slot = (self.sub_count % SHORT_TERM_SUBS as u64) as usize;
        self.ring_energy[slot] = energy;
        self.ring_len[slot] = self.frames - self.last_boundary;
        self.last_boundary = self.frames;
        self.sub_count += 1;
        self.next_boundary = (self.sub_count + 1) * u64::from(self.rate) / SUB_PER_SEC;

        if self.sub_count >= MOMENTARY_SUBS as u64 {
            let p = self.window_power(MOMENTARY_SUBS);
            self.momentary = p;
            self.momentary_max = self.momentary_max.max(p);
            let since = self.sub_count - MOMENTARY_SUBS as u64;
            if since % GATE_HOP_SUBS == 0 && p > self.abs_gate_power {
                self.gate_blocks.push(p);
            }
        }
        if self.sub_count >= SHORT_TERM_SUBS as u64 {
            let p = self.window_power(SHORT_TERM_SUBS);
            self.short_term = p;
            self.short_term_max = self.short_term_max.max(p);
            let since = self.sub_count - SHORT_TERM_SUBS as u64;
            if self.options.loudness_range && since % GATE_HOP_SUBS == 0 && p >= self.abs_gate_power
            {
                self.range_blocks.push(p);
            }
        }
    }

    /// Mean weighted square over the last `subs` sub-blocks, summed oldest first.
    fn window_power(&self, subs: usize) -> f64 {
        let (mut energy, mut len) = (0.0, 0u64);
        for j in (self.sub_count - subs as u64)..self.sub_count {
            let slot = (j % SHORT_TERM_SUBS as u64) as usize;
            energy += self.ring_energy[slot];
            len += self.ring_len[slot];
        }
        energy / len as f64
    }

    /// Momentary loudness (400 ms) of the latest window, in LUFS; `None` until 400 ms have
    /// been pushed, and for digital silence.
    pub fn momentary(&self) -> Option<f64> {
        lufs(self.momentary)
    }

    /// The highest momentary loudness seen.
    pub fn momentary_max(&self) -> Option<f64> {
        lufs(self.momentary_max)
    }

    /// Short-term loudness (3 s) of the latest window; `None` until 3 s have been pushed.
    pub fn short_term(&self) -> Option<f64> {
        lufs(self.short_term)
    }

    pub fn short_term_max(&self) -> Option<f64> {
        lufs(self.short_term_max)
    }

    /// Integrated loudness of everything pushed so far (BS.1770-4 two-stage gating); `None`
    /// when no 400 ms block is above -70 LUFS (silence, or under 400 ms of input).
    pub fn integrated(&self) -> Option<f64> {
        let n = self.gate_blocks.len();
        if n == 0 {
            return None;
        }
        let mean = self.gate_blocks.iter().sum::<f64>() / n as f64;
        // -10 LU is a factor of 0.1 in power; the +0.691 offset cancels.
        let threshold = mean * 0.1;
        let (mut sum, mut kept) = (0.0, 0usize);
        for &p in &self.gate_blocks {
            if p > threshold {
                sum += p;
                kept += 1;
            }
        }
        lufs(sum / kept as f64)
    }

    /// Loudness range in LU (EBU Tech 3342): 3 s blocks every 100 ms, absolute gate -70 LUFS,
    /// relative gate -20 LU, then the 95th minus the 10th percentile (nearest rank, as the
    /// Tech 3342 MATLAB listing rounds). `None` when disabled in [`Options`], or when no 3 s
    /// block is above the gate.
    pub fn loudness_range(&self) -> Option<f64> {
        let n = self.range_blocks.len();
        if n == 0 {
            return None;
        }
        let mean = self.range_blocks.iter().sum::<f64>() / n as f64;
        let threshold = mean * 0.01;
        let mut kept: Vec<f64> = self
            .range_blocks
            .iter()
            .copied()
            .filter(|&p| p >= threshold)
            .collect();
        kept.sort_unstable_by(f64::total_cmp);
        let rank = |percent: usize| (((kept.len() - 1) * percent + 50) / 100).min(kept.len() - 1);
        let (low, high) = (lufs(kept[rank(10)])?, lufs(kept[rank(95)])?);
        Some(high - low)
    }

    /// Largest sample magnitude per channel (linear, 1.0 is full scale); 0 for excluded channels.
    pub fn sample_peaks(&self) -> &[f64] {
        &self.sample_peaks
    }

    /// Largest sample magnitude over all channels.
    pub fn sample_peak(&self) -> f64 {
        self.sample_peaks.iter().fold(0.0, |a, &b| a.max(b))
    }

    /// True peak per channel (linear), as if the signal ended now; `None` with true peak off.
    pub fn true_peaks(&self) -> Option<Vec<f64>> {
        if !self.options.true_peak {
            return None;
        }
        Some(
            self.true_peaks
                .iter()
                .zip(&self.sample_peaks)
                .map(|((hist, peak), sample)| peak.max(*sample).max(hist.tail_peak(&self.interp)))
                .collect(),
        )
    }

    /// True peak over all channels (linear); `None` with true peak off.
    pub fn true_peak(&self) -> Option<f64> {
        Some(self.true_peaks()?.into_iter().fold(0.0, f64::max))
    }

    /// Bytes held on the heap: 8 per 100 ms of programme above the absolute gate (twice that
    /// with the loudness range on), plus a few numbers per channel. Silence costs nothing.
    pub fn heap_bytes(&self) -> usize {
        (self.gate_blocks.capacity() + self.range_blocks.capacity()) * 8
            + self.roles.capacity() * size_of::<ChannelRole>()
            + self.weights.capacity() * 8
            + self.stages.capacity() * size_of::<[Stage; 2]>()
            + self.sum_sq.capacity() * 8
            + self.sample_peaks.capacity() * 8
            + self.true_peaks.capacity() * size_of::<(History, f64)>()
    }
}

/// Mean square to LUFS; `None` for no energy.
fn lufs(power: f64) -> Option<f64> {
    (power > 0.0).then(|| OFFSET + 10.0 * libm::log10(power))
}

/// Linear amplitude to dB (dBFS or dBTP); negative infinity for zero.
pub fn to_db(linear: f64) -> f64 {
    if linear > 0.0 {
        20.0 * libm::log10(linear)
    } else {
        f64::NEG_INFINITY
    }
}
