//! A min/max pyramid of a file's samples: any time range at any pixel width without scanning
//! samples. It grows as frames arrive, so a view can draw a half-built one.
//!
//! Level 0 holds `(min, max)` as `f32` of each block of [`BASE`] frames per channel (the last
//! block partial); level k+1 merges pairs of level k, up to a single block.
//!
//! Memory is 8 bytes per block per channel per level: one hour of 48 kHz stereo is 675,000
//! level-0 blocks x 2 channels x 8 bytes = 10.8 MB, and the levels above add as much again
//! minus a block, so under 22 MB (tested).
//!
//! A query of `n` columns reads at most 4 blocks per column whatever range it spans: the
//! level chosen has blocks no wider than a column, so a column touches at most
//! `fpc / block + 2 < 4` of them (tested through [`Pyramid::columns_counting`]).

use crossfade_core::{AudioFrame, Error, Samples};

/// Frames per level-0 block.
pub const BASE: u64 = 256;

type MinMax = (f32, f32);

fn merge(a: MinMax, b: MinMax) -> MinMax {
    (a.0.min(b.0), a.1.max(b.1))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pyramid {
    channels: usize,
    frames: u64,
    /// `levels[k][channel][block]`.
    levels: Vec<Vec<Vec<MinMax>>>,
}

impl Pyramid {
    pub fn new(channels: u16) -> Self {
        let channels = usize::from(channels);
        Self {
            channels,
            frames: 0,
            levels: vec![vec![Vec::new(); channels]],
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames analysed so far; columns at or past this are `None`.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn levels(&self) -> usize {
        self.levels.len()
    }

    /// Blocks in `level` (the same for every channel).
    pub fn level_len(&self, level: usize) -> usize {
        self.levels
            .get(level)
            .and_then(|l| l.first())
            .map_or(0, Vec::len)
    }

    pub fn block(&self, level: usize, channel: usize, index: usize) -> Option<(f32, f32)> {
        self.levels.get(level)?.get(channel)?.get(index).copied()
    }

    /// Frames in one block of `level`.
    pub fn block_frames(&self, level: usize) -> u64 {
        BASE << level.min(40)
    }

    /// The coarsest level whose blocks are no wider than a column of `frames_per_column`
    /// (level 0 when columns are narrower than a block, and then a column reports its whole
    /// block).
    pub fn level_for(&self, frames_per_column: u64) -> usize {
        let ratio = (frames_per_column / BASE).max(1);
        let k = ratio.ilog2() as usize;
        k.min(self.levels.len() - 1)
    }

    /// Bytes held by one level, all channels.
    pub fn heap_bytes_level(&self, level: usize) -> usize {
        self.levels.get(level).map_or(0, |l| {
            l.iter().map(|c| c.len() * size_of::<MinMax>()).sum()
        })
    }

    pub fn heap_bytes(&self) -> usize {
        (0..self.levels.len()).map(|k| self.heap_bytes_level(k)).sum()
    }

    /// Takes the next frames in stream order. Refuses (changing nothing) a frame that is not
    /// `f64` or has another channel count. NaN counts as 0.0; infinities saturate.
    pub fn push(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F64(samples) = &frame.samples else {
            return Err(Error::Unsupported {
                what: "the waveform takes f64 frames".into(),
            });
        };
        if usize::from(frame.layout.channels()) != self.channels {
            return Err(Error::Unsupported {
                what: "the frame's channel count differs from the waveform's".into(),
            });
        }
        let ch = self.channels;
        if ch == 0 || samples.is_empty() {
            return Ok(());
        }
        let total = samples.len() / ch;
        let first_block = (self.frames / BASE) as usize;
        let mut at = 0;
        while at < total {
            let pos = self.frames + at as u64;
            let in_block = (pos % BASE) as usize;
            let n = (BASE as usize - in_block).min(total - at);
            let block = (pos / BASE) as usize;
            for c in 0..ch {
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for v in samples[at * ch + c..(at + n) * ch].iter().step_by(ch) {
                    let v = if v.is_nan() { 0.0 } else { *v as f32 };
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                let blocks = &mut self.levels[0][c];
                if blocks.len() <= block {
                    blocks.push((lo, hi));
                } else {
                    blocks[block] = merge(blocks[block], (lo, hi));
                }
            }
            at += n;
        }
        self.frames += total as u64;
        let last_block = ((self.frames - 1) / BASE) as usize;
        self.rebuild_above(first_block, last_block);
        Ok(())
    }

    /// Recomputes the parents of the level-0 blocks `from..=to` all the way up.
    fn rebuild_above(&mut self, mut from: usize, mut to: usize) {
        let mut k = 0;
        loop {
            if self.levels[k][0].len() < 2 {
                break;
            }
            if self.levels.len() == k + 1 {
                self.levels.push(vec![Vec::new(); self.channels]);
            }
            (from, to) = (from / 2, to / 2);
            for c in 0..self.channels {
                for j in from..=to {
                    let below = &self.levels[k][c];
                    let a = below[2 * j];
                    let m = below.get(2 * j + 1).map_or(a, |&b| merge(a, b));
                    let up = &mut self.levels[k + 1][c];
                    if up.len() <= j {
                        up.push(m);
                    } else {
                        up[j] = m;
                    }
                }
            }
            k += 1;
        }
    }

    /// `n` columns of `frames_per_column` frames from `start_frame` for one channel. Each is
    /// the merge of every block of the chosen level that it touches, so it is exact when the
    /// column edges fall on block edges and otherwise a little wide, never narrow. `None`
    /// from the first frame not yet analysed, or for a channel that does not exist.
    pub fn columns(
        &self,
        channel: usize,
        start_frame: u64,
        frames_per_column: u64,
        n: usize,
    ) -> Vec<Option<(f32, f32)>> {
        self.columns_counting(channel, start_frame, frames_per_column, n)
            .0
    }

    /// Like [`columns`](Self::columns), plus the number of blocks read: at most `4 * n`.
    pub fn columns_counting(
        &self,
        channel: usize,
        start_frame: u64,
        frames_per_column: u64,
        n: usize,
    ) -> (Vec<Option<(f32, f32)>>, usize) {
        let mut reads = 0;
        let cols = (0..n)
            .map(|i| self.column(&[channel], start_frame, frames_per_column, i, &mut reads))
            .collect();
        (cols, reads)
    }

    /// The envelope over all channels (lowest minimum, highest maximum): what a mixed-down
    /// display draws. Not the min/max of a true mix, which per-channel blocks cannot give.
    pub fn columns_union(
        &self,
        start_frame: u64,
        frames_per_column: u64,
        n: usize,
    ) -> Vec<Option<(f32, f32)>> {
        let all: Vec<usize> = (0..self.channels).collect();
        let mut reads = 0;
        (0..n)
            .map(|i| self.column(&all, start_frame, frames_per_column, i, &mut reads))
            .collect()
    }

    fn column(
        &self,
        channels: &[usize],
        start: u64,
        fpc: u64,
        i: usize,
        reads: &mut usize,
    ) -> Option<MinMax> {
        let fpc = fpc.max(1);
        let s = start.saturating_add((i as u64).saturating_mul(fpc));
        if s >= self.frames {
            return None;
        }
        let e = s.saturating_add(fpc).min(self.frames);
        let k = self.level_for(fpc);
        let b = self.block_frames(k);
        let (first, last) = ((s / b) as usize, ((e - 1) / b) as usize);
        let mut out: Option<MinMax> = None;
        for &c in channels {
            let blocks = self.levels[k].get(c)?;
            for m in &blocks[first..=last.min(blocks.len() - 1)] {
                *reads += 1;
                out = Some(out.map_or(*m, |o| merge(o, *m)));
            }
        }
        out
    }
}
