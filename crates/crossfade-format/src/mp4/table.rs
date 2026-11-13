//! The sample tables (`stts`, `stsc`, `stsz`/`stz2`, `stco`/`co64`) in a form that finds any
//! sample's place, size and time in O(log n): runs with their first sample precomputed, and
//! running byte totals when sample sizes vary.

use super::boxes::{Cursor, malformed};
use crossfade_core::{Error, Limits};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TimeRun {
    first_sample: u64,
    first_time: u64,
    delta: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChunkRun {
    first_sample: u64,
    /// 0-based.
    first_chunk: u64,
    per_chunk: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Sizes {
    Fixed(u32),
    /// `ends[i]`: bytes in samples `0..=i`.
    Running(Vec<u64>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SampleTable {
    count: u64,
    sizes: Sizes,
    chunks: Vec<u64>,
    chunk_runs: Vec<ChunkRun>,
    time_runs: Vec<TimeRun>,
    /// Media time at the end of the last sample.
    end_time: u64,
}

/// The raw boxes, as found in `stbl`.
#[derive(Default)]
pub(super) struct Boxes<'a> {
    pub(super) stts: Option<&'a [u8]>,
    pub(super) stsc: Option<&'a [u8]>,
    pub(super) stsz: Option<&'a [u8]>,
    pub(super) stz2: Option<&'a [u8]>,
    pub(super) stco: Option<&'a [u8]>,
    pub(super) co64: Option<&'a [u8]>,
}

fn alloc<T>(limits: &Limits, n: usize) -> Result<Vec<T>, Error> {
    limits.check_alloc(n.saturating_mul(size_of::<T>()))?;
    Ok(Vec::with_capacity(n))
}

fn sizes(b: &Boxes<'_>, limits: &Limits) -> Result<(u64, Sizes), Error> {
    if let Some(body) = b.stsz {
        let mut c = Cursor::new(body, "stsz");
        c.version()?;
        let fixed = c.u32()?;
        if fixed != 0 {
            return Ok((u64::from(c.u32()?), Sizes::Fixed(fixed)));
        }
        let n = c.count(4)?;
        let mut ends = alloc(limits, n)?;
        let mut total = 0u64;
        for _ in 0..n {
            total += u64::from(c.u32()?);
            ends.push(total);
        }
        return Ok((n as u64, Sizes::Running(ends)));
    }
    let body = b.stz2.ok_or_else(|| malformed("no sample size table"))?;
    let mut c = Cursor::new(body, "stz2");
    c.version()?;
    c.skip(3)?;
    let field = c.u8()?;
    let n = c.u32()? as usize;
    let bits = match field {
        4 | 8 | 16 => usize::from(field),
        f => return Err(malformed(format!("stz2 field size {f} is not 4, 8 or 16"))),
    };
    let packed = c.bytes(
        n.checked_mul(bits)
            .ok_or_else(|| malformed("stz2 is too big"))?
            .div_ceil(8),
    )?;
    let mut ends = alloc(limits, n)?;
    let mut total = 0u64;
    for i in 0..n {
        let size = match bits {
            4 => u64::from(packed[i / 2] >> if i % 2 == 0 { 4 } else { 0 } & 0xF),
            8 => u64::from(packed[i]),
            _ => u64::from(u16::from_be_bytes([packed[2 * i], packed[2 * i + 1]])),
        };
        total += size;
        ends.push(total);
    }
    Ok((n as u64, Sizes::Running(ends)))
}

fn chunks(b: &Boxes<'_>, limits: &Limits) -> Result<Vec<u64>, Error> {
    let (body, wide) = match (b.stco, b.co64) {
        (Some(body), _) => (body, false),
        (None, Some(body)) => (body, true),
        (None, None) => return Err(malformed("no chunk offset table")),
    };
    let mut c = Cursor::new(body, if wide { "co64" } else { "stco" });
    c.version()?;
    let n = c.count(if wide { 8 } else { 4 })?;
    let mut out = alloc(limits, n)?;
    for _ in 0..n {
        out.push(if wide { c.u64()? } else { u64::from(c.u32()?) });
    }
    Ok(out)
}

impl SampleTable {
    pub(super) fn new(b: &Boxes<'_>, limits: &Limits) -> Result<Self, Error> {
        let (count, sizes) = sizes(b, limits)?;
        let chunks = chunks(b, limits)?;

        let body = b
            .stsc
            .ok_or_else(|| malformed("no sample-to-chunk table"))?;
        let mut c = Cursor::new(body, "stsc");
        c.version()?;
        let n = c.count(12)?;
        let mut chunk_runs: Vec<ChunkRun> = alloc(limits, n)?;
        let mut covered = 0u64;
        let mut last: Option<(u64, u64)> = None;
        for i in 0..n {
            let first = u64::from(c.u32()?);
            let per_chunk = u64::from(c.u32()?);
            let description = c.u32()?;
            if description != 1 {
                return Err(Error::Unsupported {
                    what: "mp4: a track with more than one sample description is not supported"
                        .into(),
                });
            }
            if (i == 0 && first != 1) || last.is_some_and(|(f, _)| first <= f) {
                return Err(malformed("stsc's chunk numbers do not start at 1 and rise"));
            }
            if let Some((f, per)) = last {
                let to = (first - 1).min(chunks.len() as u64);
                covered = covered
                    .checked_add(to.saturating_sub(f - 1) * per)
                    .ok_or_else(|| malformed("stsc maps too many samples"))?;
            }
            last = Some((first, per_chunk));
            if per_chunk > 0 && first <= chunks.len() as u64 {
                chunk_runs.push(ChunkRun {
                    first_sample: covered,
                    first_chunk: first - 1,
                    per_chunk,
                });
            }
        }
        if let Some((f, per)) = last {
            covered = covered.saturating_add((chunks.len() as u64).saturating_sub(f - 1) * per);
        }
        if covered < count {
            return Err(malformed("the chunks hold fewer samples than stsz lists"));
        }

        let body = b.stts.ok_or_else(|| malformed("no time-to-sample table"))?;
        let mut c = Cursor::new(body, "stts");
        c.version()?;
        let n = c.count(8)?;
        let mut time_runs = alloc(limits, n)?;
        let (mut sample, mut time) = (0u64, 0u64);
        for _ in 0..n {
            let runs = u64::from(c.u32()?);
            let delta = u64::from(c.u32()?);
            if sample >= count {
                break;
            }
            let runs = runs.min(count - sample);
            if runs == 0 {
                continue;
            }
            time_runs.push(TimeRun {
                first_sample: sample,
                first_time: time,
                delta,
            });
            sample += runs;
            time = time
                .checked_add(runs * delta)
                .ok_or_else(|| malformed("stts adds up past 2^64"))?;
        }
        if sample < count {
            return Err(malformed(
                "stts gives times for fewer samples than stsz lists",
            ));
        }
        Ok(Self {
            count,
            sizes,
            chunks,
            chunk_runs,
            time_runs,
            end_time: time,
        })
    }

    pub(super) fn count(&self) -> u64 {
        self.count
    }

    /// Media time at the end of the last sample: the `stts` total.
    pub(super) fn end_time(&self) -> u64 {
        self.end_time
    }

    fn bytes_before(&self, sample: u64) -> u64 {
        match &self.sizes {
            Sizes::Fixed(size) => sample * u64::from(*size),
            Sizes::Running(ends) => match sample {
                0 => 0,
                s => ends[s as usize - 1],
            },
        }
    }

    /// Size in bytes of `sample` (below `count`).
    pub(super) fn size(&self, sample: u64) -> u64 {
        self.bytes_before(sample + 1) - self.bytes_before(sample)
    }

    /// Byte offset and size of `sample` (below `count`).
    pub(super) fn locate(&self, sample: u64) -> Result<(u64, u64), Error> {
        let run = &self.chunk_runs[self
            .chunk_runs
            .partition_point(|r| r.first_sample <= sample)
            - 1];
        let in_run = sample - run.first_sample;
        let chunk = run.first_chunk + in_run / run.per_chunk;
        let first_in_chunk = sample - in_run % run.per_chunk;
        let size = self.bytes_before(sample + 1) - self.bytes_before(sample);
        let offset = self.chunks[chunk as usize]
            .checked_add(self.bytes_before(sample) - self.bytes_before(first_in_chunk))
            .ok_or_else(|| malformed("sample offset out of range"))?;
        Ok((offset, size))
    }

    /// Media time at the start of `sample` (`count` gives the end time).
    pub(super) fn time(&self, sample: u64) -> u64 {
        if sample >= self.count {
            return self.end_time;
        }
        let run = &self.time_runs[self.time_runs.partition_point(|r| r.first_sample <= sample) - 1];
        run.first_time + (sample - run.first_sample) * run.delta
    }

    /// The last sample whose `key(time)` is at or before `target` (0 when none is): `key` must
    /// not decrease with time.
    pub(super) fn sample_at(&self, target: i64, key: impl Fn(u64) -> i64) -> u64 {
        let (mut lo, mut hi) = (0u64, self.count);
        // Invariant: every sample below `lo` is at or before `target`, none from `hi` on.
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if key(self.time(mid)) <= target {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo.saturating_sub(1)
    }
}
