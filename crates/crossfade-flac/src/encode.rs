//! A FLAC encoder (RFC 9639): interleaved samples in, one frame per block out.
//!
//! Fixed block size, 4 to 32 bits per sample, 1 to 8 channels, any rate FLAC can name. Every
//! frame decodes bit-exactly with this crate's [`Decoder`](crate::Decoder), and the final
//! STREAMINFO carries the MD5 of the audio, so `Decoder::finish` returns `Md5Matched`.
//!
//! ```
//! use crossfade_flac::{Decoder, Encoder, EncoderConfig, Verification};
//!
//! let mut encoder = Encoder::new(EncoderConfig::new(44_100, 2, 16))?;
//! let samples: Vec<i32> = (0..2 * 5000).map(|i| (i % 200) * 50 - 5000).collect();
//! let mut frames = Vec::new();
//! let mut ends = Vec::new(); // where each frame ends in `frames`
//! // One call per block; the last may be short. (`write` and `flush` take any chunk size.)
//! for block in samples.chunks(encoder.block_size() * 2) {
//!     encoder.encode_block(block, &mut frames)?;
//!     ends.push(frames.len());
//! }
//! let info = encoder.finish(); // goes in the STREAMINFO block of the file
//!
//! let mut decoder = Decoder::new(&info)?;
//! let mut start = 0;
//! for end in ends {
//!     decoder.decode_frame(&frames[start..end])?;
//!     start = end;
//! }
//! assert_eq!(decoder.finish()?, Verification::Md5Matched);
//! # Ok::<(), crossfade_flac::Error>(())
//! ```
//!
//! # Levels
//!
//! `level` runs from 0 (fastest) to 8 (smallest); each row adds to the ones above it. The
//! sizes track the reference encoder's presets at the same level (see the crate README).
//!
//! | level | block | stereo | fixed orders coded | LPC max order | LPC windows (orders tried, coded) | max Rice partition order |
//! |---|---|---|---|---|---|---|
//! | 0 | 1152 | independent | all 5 | none | none | 3 |
//! | 1 | 1152 | adaptive | all 5 | none | none | 3 |
//! | 2 | 1152 | exhaustive | all 5 | none | none | 3 |
//! | 3 | 4096 | adaptive | best 2 | 6 | full (1, 1) | 4 |
//! | 4 | 4096 | adaptive | best 2 | 8 | full (1, 1) | 4 |
//! | 5 | 4096 | exhaustive | best 2 | 8 | full (2, 2) | 5 |
//! | 6 | 4096 | exhaustive | best 2 | 8 | full (3, 3) | 6 |
//! | 7 | 4096 | exhaustive | best 2 | 12 | full (4, 2) | 6 |
//! | 8 | 4096 | exhaustive | best 2 | 12 | full (3, 1), first and last two thirds (1, 1 each) | 6 |
//!
//! Every level also does the following. The candidates that get coded are compared on exact
//! bit counts; where a level tries more than it codes, the ones coded are the best by a
//! prediction:
//!
//! - **Constant and verbatim subframes**, and **wasted bits**: low zero bits common to every
//!   sample of a channel are shifted out and signalled.
//! - **Fixed predictors** of order 0 to 4. Levels 0 to 2 code all five and keep the cheapest.
//!   From level 3 a one-pass estimate (the sum of absolute residuals) picks the two best
//!   orders to code, because linear prediction usually wins anyway.
//! - **Linear prediction** from level 3: a window (Tukey-like, tapered with a smoothstep
//!   polynomial), autocorrelation, one Levinson-Durbin run for every order up to the maximum,
//!   coefficients quantised with the largest shift that fits. The precision follows the
//!   reference encoder: below 16 bits `2 + depth / 2` (at least 5); at 16 bits 7 to 13 by block
//!   size (12 at 4096); above 16 bits 12, 13, then 15 by block size (15 at 4096); never over
//!   15. The best few orders by the reference encoder's size estimate (the first count in the
//!   table) are tried. Where a level codes fewer than it tries, each is scored by the error
//!   energy the block's covariance predicts for it (an exact quadratic form over a covariance
//!   built from one integer autocorrelation, no pass over the block per candidate) and the best
//!   by that score are coded for real (the second count); the cheapest coded wins.
//!   A coded fit that lands more than 3.5 bits per sample above its own estimate (predictable,
//!   band-limited audio at a high rate, where coefficients of a wide dynamic range quantise
//!   coarsely) gets a closer look: a search for a better integer point near the rounded
//!   coefficients, and from level 5 a few more coefficient sets from a ridge added to the
//!   autocorrelation (2 extra amounts at levels 5 and 6, 3 at 7, 4 at 8), each rounded and
//!   refined. All of those are scored the same way and the best one is coded; and a window
//!   whose coded fits all land that far above their estimates codes the rest of the orders it
//!   tried, since there the prediction ranks them less surely. Ordinary audio never gets
//!   there, so it pays nothing for it. (Samples too wide for an exact covariance, loud
//!   audio in a 32-bit stream, code every candidate they try.)
//! - **Rice coding** with the partition order searched from 0 up to the level's maximum (and
//!   as far as the block size and predictor order allow). Each partition's parameter is the
//!   exact optimum; 5-bit parameters are used when a partition wants one above 14 and they
//!   pay for themselves, and a partition is stored raw (escaped) when that is cheaper.
//! - **Stereo**: level 0 codes channels independently. *Adaptive* levels rank left/side,
//!   side/right and mid/side by a cheap estimate and code only the winning pair;
//!   *exhaustive* levels code left, right, mid and side fully and keep the cheapest pair.
//!   Either way the pair is compared with plain coding by exact bits before it is emitted.
//!
//! # Guarantees
//!
//! - **Never worse than raw.** A frame is never larger than one made of independent verbatim
//!   (or constant) subframes: a verbatim candidate is always in the race, and a decorrelated
//!   pair that loses to it by exact bits is replaced.
//! - **The format's limits hold.** A residual must fit 32-bit two's complement and not be the
//!   most negative value; a predictor or stereo mode that would break that is passed over, down
//!   to verbatim, which always works. The side channel of a 32-bit stream is 33 bits wide and
//!   is handled in `i64`.
//! - **Deterministic.** A frame depends only on its block's samples, its number and the
//!   configuration (no analyser state carries from one block to the next in a way that shows
//!   in the output), so [`EncoderConfig::threads`] never changes a byte: every thread codes
//!   with its own analyser, frames come out in order, and the MD5 sees the samples in order.
//!   The autocorrelation and covariance sums are exact
//!   integers wherever they fit (any order of summing gives the same result, so the compiler
//!   may vectorise them). Floating point is used only for the window, the autocorrelation of
//!   samples too wide for that, Levinson-Durbin, quantisation, refinement and the size
//!   estimates, through operations IEEE 754 defines exactly (`+ - * /`, comparisons,
//!   conversions) in an order fixed in the source: no `sin`, `cos`, `exp`, `ln`, `powf` or
//!   fused multiply-add, whose results differ between platforms' maths libraries. The same
//!   input and configuration give the same bytes everywhere.
//! - **Fast in any release profile.** The hot loops rely on the compiler's auto-vectorisation
//!   (the crate has no `unsafe`, so no hand-written SIMD): the lagged sums of the integer
//!   autocorrelation and covariance (`lpc::four_lags`), the order-specialised LPC residual
//!   kernels (`lpc::kernel`), and the fixed-order residuals and estimate sums
//!   (`fixed::residual`, `fixed::abs_sums_narrow`) are each a function of its own marked
//!   `#[inline(never)]`, looping over zipped, windowed or chunked slices with no indexing
//!   inside, so the loop the compiler sees, and whether a bounds check survives in it, does
//!   not depend on what it is inlined into. (The Rice search's masked sums are plain folds over
//!   one slice, which need no such care.) One codegen unit or LTO changes inlining: the lagged
//!   sums once compiled to a scalar loop there, and levels 5 to 8 ran up to twice as slow.
//!   Checked by building with `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1` and with thin and fat
//!   LTO, and comparing the disassembly of those functions (`smlal`/`mla.4s` present) and the
//!   encode times.
//! - **No panics, bounded memory.** One block of working memory, allocated in
//!   [`Encoder::new`] and reused; steady state allocates nothing but what the caller's `out`
//!   needs. With threads, each worker has its own block of working memory and at most
//!   `2 * threads` blocks are in flight (plus a few waiting for the MD5 thread), their buffers
//!   reused. A panic on a worker (a bug) is [`Error::Internal`] from
//!   then on, never a hang. Arithmetic on sample data is either checked against the limits or
//!   wraps where the result is known to fit.

mod analyze;
mod bitsink;
mod fixed;
mod lpc;
mod rice;
mod write;

use crate::error::{Error, Result};
use crate::frame::{self, ChannelAssignment, FrameHeader};
use crate::md5::Md5;
#[cfg(test)]
use crate::pool::auto_threads;
use crate::pool::{Ordered, Panicked, Serial};
use crate::streaminfo::StreamInfoBlock;
use analyze::{Analyzer, Level, Plan, Stereo, is_constant};
use bitsink::BitSink;

/// What to encode and how hard to try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    /// 1 to 1_048_575.
    pub sample_rate: u32,
    /// 1 to 8.
    pub channels: u8,
    /// 4 to 32.
    pub bits_per_sample: u8,
    /// 0 (fastest) to 8 (smallest); 5 is the default.
    pub level: u8,
    /// How many threads may code frames at once, the calling thread included; it never
    /// changes the output. `1` (what [`new`](Self::new) sets) codes every frame on the calling
    /// thread and starts no thread at all. More lets [`write`](Encoder::write) hand blocks to
    /// up to `threads - 1` worker threads, started only as blocks queue up, and hash the audio
    /// on one more; frames then come out a few blocks later (see
    /// [`frames_in_flight`](Encoder::frames_in_flight)). [`AUTO_THREADS`](Self::AUTO_THREADS)
    /// (0) takes the machine's parallelism less three, at least 2 and at most
    /// [`MAX_AUTO_THREADS`](Self::MAX_AUTO_THREADS): the MD5 thread, and the caller's reading
    /// and writing, need cores of their own.
    /// Values above [`MAX_THREADS`](Self::MAX_THREADS) count as that.
    pub threads: usize,
}

impl EncoderConfig {
    pub const DEFAULT_LEVEL: u8 = 5;
    /// `threads` value that means "as many as the machine runs at once".
    pub const AUTO_THREADS: usize = 0;
    /// The most [`AUTO_THREADS`](Self::AUTO_THREADS) picks: past it the MD5 thread, not the
    /// frames, sets the pace.
    pub const MAX_AUTO_THREADS: usize = crate::pool::MAX_AUTO_THREADS;
    /// The most threads an encoder uses, whatever `threads` says.
    pub const MAX_THREADS: usize = 64;

    pub fn new(sample_rate: u32, channels: u8, bits_per_sample: u8) -> Self {
        Self {
            sample_rate,
            channels,
            bits_per_sample,
            level: Self::DEFAULT_LEVEL,
            threads: 1,
        }
    }

    /// The thread count `threads` stands for on this machine.
    fn resolved_threads(&self) -> usize {
        match self.threads {
            Self::AUTO_THREADS => crate::auto_thread_count(),
            n => n.min(Self::MAX_THREADS),
        }
    }
}

/// Blocks in flight per thread that codes frames: enough that a worker always finds the next
/// one waiting, few enough that memory stays at a handful of blocks.
const DEPTH: usize = 2;
/// Blocks waiting for the MD5 thread before the caller waits for it.
const MD5_DEPTH: usize = 8;

fn malformed(what: &str) -> Error {
    Error::Malformed(format!("flac encoder: {what}"))
}

fn unsupported(what: &str) -> Error {
    Error::Unsupported(format!("flac encoder: {what}"))
}

/// Fixed-block-size FLAC encoder; see the module documentation.
pub struct Encoder {
    config: EncoderConfig,
    block: usize,
    frame_number: u64,
    total: u64,
    min_frame: u32,
    max_frame: u32,
    short_seen: bool,
    done: Option<StreamInfoBlock>,
    /// Samples per channel in the frames appended to an `out` so far; `total` also counts the
    /// frames in flight.
    written: u64,
    /// The MD5 while it runs on the calling thread; it moves to `md5_stage` once workers start.
    hasher: Hasher,
    /// The MD5 on a thread of its own, fed each block's packed bytes as the block goes to the
    /// pool: it is the one stage that cannot be split, so it starts as early as it can.
    md5_stage: Option<Serial<Md5, Vec<u8>>>,
    /// Samples taken by `write` that do not fill a block yet.
    pending: Vec<i32>,
    /// The calling thread's coder; with workers it codes blocks while it waits for a frame.
    coder: FrameCoder,
    /// Resolved `config.threads`.
    threads: usize,
    /// Created by the first block `write` hands over when `threads > 1`.
    pool: Option<Ordered<Job, Job, FrameCoder>>,
    /// Jobs to reuse, with their buffers.
    spare_jobs: Vec<Job>,
    /// A worker panicked: every later call says so.
    failed: Option<Error>,
    /// Makes the coder of frame N panic, for the failure tests.
    #[cfg(test)]
    panic_at: Option<u64>,
}

/// One block on its way through the pool: its number and samples in, its frame out.
struct Job {
    number: u64,
    samples: Vec<i32>,
    frame: Vec<u8>,
}

fn code_job(coder: &mut FrameCoder, mut job: Job) -> Job {
    job.frame.clear();
    coder.encode(&job.samples, job.number, &mut job.frame);
    job
}

/// The samples as the MD5 takes them: little-endian integers in the smallest whole number of
/// bytes the depth fits.
fn pack(bits_per_sample: u8, interleaved: &[i32], out: &mut Vec<u8>) {
    fn pack<const W: usize>(out: &mut Vec<u8>, samples: &[i32]) {
        out.clear();
        out.resize(samples.len() * W, 0);
        for (dst, &s) in out.as_chunks_mut::<W>().0.iter_mut().zip(samples) {
            dst.copy_from_slice(&s.to_le_bytes()[..W]);
        }
    }
    match bits_per_sample.div_ceil(8) {
        1 => pack::<1>(out, interleaved),
        2 => pack::<2>(out, interleaved),
        3 => pack::<3>(out, interleaved),
        _ => pack::<4>(out, interleaved),
    }
}

/// The MD5 of the audio on the calling thread, with its packing buffer.
struct Hasher {
    md5: Md5,
    bytes: Vec<u8>,
    bits_per_sample: u8,
}

impl Hasher {
    fn update(&mut self, interleaved: &[i32]) {
        pack(self.bits_per_sample, interleaved, &mut self.bytes);
        self.md5.update(&self.bytes);
    }
}

#[allow(clippy::ptr_arg)] // the serial stage passes a reference to its item type
fn hash_bytes(md5: &mut Md5, bytes: &Vec<u8>) {
    md5.update(bytes);
}

fn panicked(p: Panicked) -> Error {
    Error::Internal(format!(
        "flac encoder: a worker thread panicked ({}); the stream cannot continue",
        p.0
    ))
}

/// Everything one frame needs beyond its samples and number: the block's working memory and
/// the analyser. Nothing in it carries over from one block to the next in a way that changes
/// the output, so frames can be coded by any number of these in any order.
struct FrameCoder {
    config: EncoderConfig,
    /// One block, deinterleaved.
    planes: Vec<Vec<i32>>,
    /// Mid and side of a stereo block; the side of a 32-bit stream needs 33 bits.
    mid: Vec<i32>,
    side32: Vec<i32>,
    side64: Vec<i64>,
    /// One plan per channel; a stereo stream has four: left, right, mid, side.
    slots: Vec<Plan>,
    level: Level,
    analyzer: Analyzer,
    #[cfg(test)]
    force: Option<ChannelAssignment>,
    #[cfg(test)]
    panic_at: Option<u64>,
}

impl Encoder {
    pub fn new(config: EncoderConfig) -> Result<Self> {
        if !(1..=1_048_575).contains(&config.sample_rate) {
            return Err(unsupported("sample rate must be 1 to 1048575 Hz"));
        }
        if !(1..=8).contains(&config.channels) {
            return Err(unsupported("channel count must be 1 to 8"));
        }
        if !(4..=32).contains(&config.bits_per_sample) {
            return Err(unsupported("bit depth must be 4 to 32"));
        }
        if config.level > 8 {
            return Err(unsupported("level must be 0 to 8"));
        }
        let level = Level::new(config.level);
        let channels = usize::from(config.channels);
        Ok(Self {
            config,
            block: level.block,
            frame_number: 0,
            total: 0,
            min_frame: u32::MAX,
            max_frame: 0,
            short_seen: false,
            done: None,
            written: 0,
            hasher: Hasher {
                md5: Md5::new(),
                bytes: Vec::with_capacity(level.block * channels * 4),
                bits_per_sample: config.bits_per_sample,
            },
            md5_stage: None,
            pending: Vec::with_capacity(level.block * channels),
            coder: FrameCoder::new(config, level),
            threads: config.resolved_threads(),
            pool: None,
            spare_jobs: Vec::new(),
            failed: None,
            #[cfg(test)]
            panic_at: None,
        })
    }

    /// Samples per channel in every frame but the last.
    pub fn block_size(&self) -> usize {
        self.block
    }

    /// Frames [`write`](Self::write) has taken blocks for but not appended to an `out` yet:
    /// always 0 with one thread, at most `2 * threads` with more. [`flush`](Self::flush)
    /// appends them all.
    pub fn frames_in_flight(&self) -> usize {
        self.pool.as_ref().map_or(0, Ordered::in_flight)
    }

    /// Refuses a call on a stream that is finished or failed, or past its short block.
    fn check_open(&self) -> Result<()> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        if self.done.is_some() {
            return Err(malformed("the stream is finished"));
        }
        if self.short_seen {
            return Err(malformed("a short block must be the last"));
        }
        Ok(())
    }

    /// Encodes one block and appends exactly one FLAC frame to `out`, always on the calling
    /// thread (only [`write`](Self::write) uses worker threads). `interleaved` holds raw
    /// sign-extended samples, `block_size() * channels` of them, or fewer (a whole number of
    /// sample frames, at least one) for the final block; after a short block no more may
    /// follow. A sample outside the configured depth is `Error::Malformed`, and so is a call
    /// while `write` has samples pending or frames in flight ([`flush`](Self::flush) first).
    pub fn encode_block(&mut self, interleaved: &[i32], out: &mut Vec<u8>) -> Result<()> {
        self.check_open()?;
        let channels = usize::from(self.config.channels);
        if interleaved.is_empty() || !interleaved.len().is_multiple_of(channels) {
            return Err(malformed(
                "a block is a whole number of sample frames, at least one",
            ));
        }
        let n = interleaved.len() / channels;
        if n > self.block {
            return Err(malformed("block is longer than block_size()"));
        }
        if !self.pending.is_empty() {
            return Err(malformed("samples from write() are pending; flush() first"));
        }
        if self.frames_in_flight() > 0 {
            return Err(malformed(
                "frames from write() are in flight; flush() first",
            ));
        }
        self.check_range(interleaved)?;
        self.check_length(n)?;

        self.hash(interleaved)?;
        let start = out.len();
        self.coder.encode(interleaved, self.frame_number, out);
        self.wrote((out.len() - start) as u32, n);
        self.frame_number += 1;
        self.total += n as u64;
        self.short_seen = n < self.block;
        Ok(())
    }

    fn check_length(&self, n: usize) -> Result<()> {
        if self.frame_number >= 1 << 31 || self.total + n as u64 >= 1 << 36 {
            return Err(unsupported("stream is longer than FLAC can number"));
        }
        Ok(())
    }

    /// Counts a frame of `len` bytes and `n` samples per channel that reached an `out`.
    fn wrote(&mut self, len: u32, n: usize) {
        self.min_frame = self.min_frame.min(len);
        self.max_frame = self.max_frame.max(len);
        self.written += n as u64;
    }

    fn check_range(&self, samples: &[i32]) -> Result<()> {
        let bps = u32::from(self.config.bits_per_sample);
        if bps < 32 {
            let (lo, hi) = (-(1i32 << (bps - 1)), (1i32 << (bps - 1)) - 1);
            // A min/max fold vectorises; an early-exit `any` does not.
            let (min, max) = samples
                .iter()
                .fold((i32::MAX, i32::MIN), |(a, b), &s| (a.min(s), b.max(s)));
            if min < lo || max > hi {
                return Err(malformed("sample outside the configured bit depth"));
            }
        }
        Ok(())
    }

    /// Takes samples in chunks of any size (whole sample frames) and appends a frame to `out`
    /// each time a block fills; with more than one thread, each time a frame is ready, which
    /// may be a few blocks later (in order all the same). Call [`flush`](Self::flush) at the
    /// end for the remainder and the frames in flight. Do not mix with
    /// [`encode_block`](Self::encode_block) until a flush.
    pub fn write(&mut self, interleaved: &[i32], out: &mut Vec<u8>) -> Result<()> {
        self.write_inner(interleaved, out, None)
    }

    /// As [`write`](Self::write), and pushes `out.len()` onto `ends` after each frame it
    /// appends, so a caller can cut `out` into frames (one packet per frame) without parsing.
    pub fn write_ends(
        &mut self,
        interleaved: &[i32],
        out: &mut Vec<u8>,
        ends: &mut Vec<usize>,
    ) -> Result<()> {
        self.write_inner(interleaved, out, Some(ends))
    }

    fn write_inner(
        &mut self,
        mut interleaved: &[i32],
        out: &mut Vec<u8>,
        mut ends: Option<&mut Vec<usize>>,
    ) -> Result<()> {
        self.check_open()?;
        let channels = usize::from(self.config.channels);
        if !interleaved.len().is_multiple_of(channels) {
            return Err(malformed("write() takes whole sample frames"));
        }
        self.check_range(interleaved)?;
        let per = self.block * channels;
        if self.threads > 1 {
            if !self.pending.is_empty() {
                let take = (per - self.pending.len()).min(interleaved.len());
                self.pending.extend_from_slice(&interleaved[..take]);
                interleaved = &interleaved[take..];
                if self.pending.len() == per {
                    let job = self.job_from_pending();
                    self.submit(job, out, ends.as_deref_mut())?;
                }
            }
            while interleaved.len() >= per {
                let (block, rest) = interleaved.split_at(per);
                let mut job = self.spare_job();
                job.samples.extend_from_slice(block);
                self.submit(job, out, ends.as_deref_mut())?;
                interleaved = rest;
            }
            self.pending.extend_from_slice(interleaved);
            return self.take_ready(out, ends);
        }
        if !self.pending.is_empty() {
            let take = (per - self.pending.len()).min(interleaved.len());
            self.pending.extend_from_slice(&interleaved[..take]);
            interleaved = &interleaved[take..];
            if self.pending.len() == per {
                let full = std::mem::take(&mut self.pending);
                let result = self.encode_block(&full, out);
                self.pending = full;
                self.pending.clear();
                result?;
                if let Some(e) = ends.as_deref_mut() {
                    e.push(out.len());
                }
            }
        }
        while interleaved.len() >= per {
            let (block, rest) = interleaved.split_at(per);
            self.encode_block(block, out)?;
            if let Some(e) = ends.as_deref_mut() {
                e.push(out.len());
            }
            interleaved = rest;
        }
        self.pending.extend_from_slice(interleaved);
        Ok(())
    }

    /// Encodes the samples [`write`](Self::write) still holds as the final (short) block. Does
    /// nothing when none are pending. Call it before [`finish`](Self::finish): samples left
    /// pending then are not part of the stream.
    pub fn flush(&mut self, out: &mut Vec<u8>) -> Result<()> {
        self.flush_inner(out, None)
    }

    /// As [`flush`](Self::flush), reporting the frame's end like [`write_ends`](Self::write_ends).
    pub fn flush_ends(&mut self, out: &mut Vec<u8>, ends: &mut Vec<usize>) -> Result<()> {
        self.flush_inner(out, Some(ends))
    }

    fn flush_inner(&mut self, out: &mut Vec<u8>, mut ends: Option<&mut Vec<usize>>) -> Result<()> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        if self.threads > 1 && (self.pool.is_some() || !self.pending.is_empty()) {
            if !self.pending.is_empty() {
                self.check_open()?;
                let job = self.job_from_pending();
                self.submit_with(job, out, ends.as_deref_mut(), true)?;
            }
            return self.take_all(out, ends);
        }
        if self.pending.is_empty() {
            return Ok(());
        }
        let rest = std::mem::take(&mut self.pending);
        let result = self.encode_block(&rest, out);
        self.pending = rest;
        self.pending.clear();
        result?;
        if let Some(e) = ends {
            e.push(out.len());
        }
        Ok(())
    }

    /// Adds a block to the MD5, on its thread if it has one.
    fn hash(&mut self, interleaved: &[i32]) -> Result<()> {
        let Some(stage) = &mut self.md5_stage else {
            self.hasher.update(interleaved);
            return Ok(());
        };
        let mut bytes = stage.spare().unwrap_or_default();
        pack(self.config.bits_per_sample, interleaved, &mut bytes);
        let sent = stage.send(bytes);
        sent.map_err(|p| self.failed_with(p))
    }

    /// A job with empty buffers, reused where one is free.
    fn spare_job(&mut self) -> Job {
        let mut job = self.spare_jobs.pop().unwrap_or_else(|| Job {
            number: 0,
            samples: Vec::with_capacity(self.block * usize::from(self.config.channels)),
            frame: Vec::new(),
        });
        job.samples.clear();
        job
    }

    /// A job holding the pending samples; `pending` takes the job's empty buffer.
    fn job_from_pending(&mut self) -> Job {
        let mut job = self.spare_job();
        std::mem::swap(&mut job.samples, &mut self.pending);
        job
    }

    /// Records a failure: the threads stop and every later call returns it.
    fn failed_with(&mut self, p: Panicked) -> Error {
        let e = panicked(p);
        self.failed = Some(e.clone());
        // Dropping them stops and joins the threads.
        self.pool = None;
        self.md5_stage = None;
        e
    }

    /// Hands a block (whole sample frames, at most a block) to the pool, first taking frames
    /// out while it is full.
    fn submit(&mut self, job: Job, out: &mut Vec<u8>, ends: Option<&mut Vec<usize>>) -> Result<()> {
        self.submit_with(job, out, ends, false)
    }

    /// `last`: the caller drains right after, so the block is not worth a new thread.
    fn submit_with(
        &mut self,
        mut job: Job,
        out: &mut Vec<u8>,
        mut ends: Option<&mut Vec<usize>>,
        last: bool,
    ) -> Result<()> {
        let n = job.samples.len() / usize::from(self.config.channels);
        self.check_length(n)?;
        if self.pool.is_none() {
            let (config, level) = (self.config, self.coder.level);
            #[cfg(test)]
            let panic_at = self.panic_at;
            #[cfg(test)]
            {
                self.coder.panic_at = panic_at;
            }
            self.pool = Some(Ordered::new(
                self.threads - 1,
                DEPTH,
                move || {
                    #[allow(unused_mut)]
                    let mut coder = FrameCoder::new(config, level);
                    #[cfg(test)]
                    {
                        coder.panic_at = panic_at;
                    }
                    coder
                },
                code_job,
            ));
        }
        while self.pool.as_ref().is_some_and(Ordered::is_full) {
            self.take_one(out, ends.as_deref_mut())?;
        }
        job.number = self.frame_number;
        self.hash(&job.samples)?;
        let Some(pool) = &mut self.pool else {
            unreachable!("made above")
        };
        if last {
            pool.submit_last(job);
        } else {
            pool.submit(job);
        }
        let started = pool.workers() > 0;
        self.frame_number += 1;
        self.total += n as u64;
        self.short_seen = n < self.block;
        if started && self.md5_stage.is_none() {
            // Workers are running, so the stream is long enough for MD5 to pay for a thread.
            let md5 = std::mem::take(&mut self.hasher.md5);
            match Serial::new(md5, hash_bytes, MD5_DEPTH) {
                Ok(stage) => self.md5_stage = Some(stage),
                Err(md5) => self.hasher.md5 = md5,
            }
        }
        Ok(())
    }

    /// Appends a frame the pool has finished.
    fn emit(&mut self, job: Job, out: &mut Vec<u8>, ends: Option<&mut Vec<usize>>) -> Result<()> {
        out.extend_from_slice(&job.frame);
        if let Some(e) = ends {
            e.push(out.len());
        }
        let n = job.samples.len() / usize::from(self.config.channels);
        self.wrote(job.frame.len() as u32, n);
        self.spare_jobs.push(job);
        Ok(())
    }

    /// Appends the frames that are ready, in order, without waiting.
    fn take_ready(&mut self, out: &mut Vec<u8>, mut ends: Option<&mut Vec<usize>>) -> Result<()> {
        while let Some(result) = self.pool.as_mut().and_then(Ordered::try_next) {
            let job = result.map_err(|p| self.failed_with(p))?;
            self.emit(job, out, ends.as_deref_mut())?;
        }
        Ok(())
    }

    /// Waits for the next frame in order (coding blocks meanwhile) and appends it.
    fn take_one(&mut self, out: &mut Vec<u8>, ends: Option<&mut Vec<usize>>) -> Result<()> {
        let next = match &mut self.pool {
            Some(pool) => pool.next(&mut self.coder),
            None => None,
        };
        match next {
            Some(Ok(job)) => self.emit(job, out, ends),
            Some(Err(p)) => Err(self.failed_with(p)),
            None => Ok(()),
        }
    }

    fn take_all(&mut self, out: &mut Vec<u8>, mut ends: Option<&mut Vec<usize>>) -> Result<()> {
        loop {
            let next = match &mut self.pool {
                Some(pool) => pool.next(&mut self.coder),
                None => None,
            };
            match next {
                Some(Ok(job)) => self.emit(job, out, ends.as_deref_mut())?,
                Some(Err(p)) => return Err(self.failed_with(p)),
                // Nothing in flight (or a closed pool): never spin on a count.
                None => return Ok(()),
            }
        }
    }

    fn info(&self) -> StreamInfoBlock {
        StreamInfoBlock {
            min_block_size: self.block as u16,
            max_block_size: self.block as u16,
            min_frame_size: 0,
            max_frame_size: 0,
            sample_rate: self.config.sample_rate,
            channels: self.config.channels,
            bits_per_sample: self.config.bits_per_sample,
            total_samples: 0,
            md5: [0; 16],
        }
    }

    /// Provisional: block sizes and format only; frame sizes, total and MD5 are zero
    /// (unknown) until [`finish`](Self::finish), whatever the thread count.
    pub fn stream_info(&self) -> StreamInfoBlock {
        self.done.unwrap_or_else(|| self.info())
    }

    /// Ends the stream and returns the final STREAMINFO (minimum and maximum frame size,
    /// total samples, MD5) of the frames appended to an `out`, and joins any threads. Frames
    /// still in flight are dropped, as pending samples are: [`flush`](Self::flush) first. The
    /// MD5 is taken as blocks go to the workers, so if frames in flight are dropped it is left
    /// zero ("unknown" to a decoder). Further blocks are refused; calling it again returns the
    /// same.
    pub fn finish(&mut self) -> StreamInfoBlock {
        if let Some(info) = self.done {
            return info;
        }
        let dropped = self.frames_in_flight() > 0;
        self.pool = None;
        let mut info = self.info();
        if self.written > 0 {
            info.min_frame_size = self.min_frame;
            info.max_frame_size = self.max_frame;
        }
        info.total_samples = self.written;
        info.md5 = match self.md5_stage.take().map(Serial::finish) {
            None => std::mem::take(&mut self.hasher.md5).finalize(),
            Some(Ok(md5)) => md5.finalize(),
            // Zero means "not computed" to a decoder; the thread cannot panic in practice.
            Some(Err(_)) => [0; 16],
        };
        if dropped {
            info.md5 = [0; 16];
        }
        self.done = Some(info);
        info
    }
}

impl FrameCoder {
    fn new(config: EncoderConfig, level: Level) -> Self {
        let channels = usize::from(config.channels);
        Self {
            config,
            planes: vec![vec![0; level.block]; channels],
            mid: Vec::with_capacity(if channels == 2 { level.block } else { 0 }),
            side32: Vec::with_capacity(if channels == 2 { level.block } else { 0 }),
            side64: Vec::with_capacity(if channels == 2 && config.bits_per_sample == 32 {
                level.block
            } else {
                0
            }),
            slots: (0..if channels == 2 { 4 } else { channels })
                .map(|_| Plan::new(level.block))
                .collect(),
            level,
            analyzer: Analyzer::new(
                level,
                u32::from(config.bits_per_sample),
                channels == 2 && config.bits_per_sample == 32,
            ),
            #[cfg(test)]
            force: None,
            #[cfg(test)]
            panic_at: None,
        }
    }

    /// Appends frame `number` coding `interleaved`, a checked block of whole sample frames.
    fn encode(&mut self, interleaved: &[i32], number: u64, out: &mut Vec<u8>) {
        #[cfg(test)]
        if self.panic_at == Some(number) {
            panic!("test panic at frame {number}");
        }
        let channels = usize::from(self.config.channels);
        let n = interleaved.len() / channels;
        for plane in &mut self.planes {
            plane.resize(n, 0);
        }
        match self.planes.as_mut_slice() {
            [mono] => mono.copy_from_slice(interleaved),
            [left, right] => {
                for ((l, r), f) in left
                    .iter_mut()
                    .zip(right)
                    .zip(interleaved.as_chunks::<2>().0)
                {
                    (*l, *r) = (f[0], f[1]);
                }
            }
            planes => {
                for (i, frame) in interleaved.chunks_exact(channels).enumerate() {
                    for (plane, &v) in planes.iter_mut().zip(frame) {
                        plane[i] = v;
                    }
                }
            }
        }
        self.write_frame(n, number, out);
    }

    /// Plans one channel of the block into `slot`: a plane, or for a stereo stream slot 2
    /// is the mid and slot 3 the side channel.
    fn analyze_slot(&mut self, slot: usize, n: usize, bps: u32) {
        let Self {
            analyzer,
            slots,
            planes,
            mid,
            side32,
            side64,
            ..
        } = self;
        let plan = &mut slots[slot];
        if slot < planes.len() {
            analyzer.analyze(&planes[slot][..n], bps, plan);
        } else if slot == 2 {
            analyzer.analyze(&mid[..n], bps, plan);
        } else if bps < 32 {
            analyzer.analyze(&side32[..n], bps + 1, plan);
        } else {
            analyzer.analyze(&side64[..n], 33, plan);
        }
    }

    fn estimate_slot(&self, slot: usize, n: usize, bps: u32) -> u64 {
        if slot < self.planes.len() {
            fixed::estimate(&self.planes[slot][..n])
        } else if slot == 2 {
            fixed::estimate(&self.mid[..n])
        } else if bps < 32 {
            fixed::estimate(&self.side32[..n])
        } else {
            fixed::estimate(&self.side64[..n])
        }
    }

    /// Picks the channel assignment of a stereo block and plans the two channels it uses.
    fn analyze_stereo(&mut self, n: usize, bps: u32) -> ChannelAssignment {
        self.mid.clear();
        self.side32.clear();
        self.side64.clear();
        for (&l, &r) in self.planes[0][..n].iter().zip(&self.planes[1][..n]) {
            let (l, r) = (i64::from(l), i64::from(r));
            self.mid.push(((l + r) >> 1) as i32);
            if bps < 32 {
                self.side32.push((l - r) as i32);
            } else {
                self.side64.push(l - r);
            }
        }
        // Slots: left, right, mid, side.
        let modes = [
            (ChannelAssignment::Independent(2), [0, 1]),
            (ChannelAssignment::LeftSide, [0, 3]),
            (ChannelAssignment::SideRight, [3, 1]),
            (ChannelAssignment::MidSide, [2, 3]),
        ];
        #[cfg(test)]
        if let Some(mode) = self.force {
            let pair = modes.iter().find(|m| m.0 == mode).map_or([0, 1], |m| m.1);
            for slot in pair {
                self.analyze_slot(slot, n, bps);
            }
            return mode;
        }
        let pick = |cost: &[u64; 4]| {
            let mut best = 0;
            for m in 1..modes.len() {
                let total = |i: usize| cost[modes[i].1[0]] + cost[modes[i].1[1]];
                if total(m) < total(best) {
                    best = m;
                }
            }
            best
        };
        let mut cost = [0u64; 4];
        let best = if self.level.stereo == Stereo::Exhaustive {
            for (slot, c) in cost.iter_mut().enumerate() {
                self.analyze_slot(slot, n, bps);
                *c = self.slots[slot].bits;
            }
            pick(&cost)
        } else {
            for (slot, c) in cost.iter_mut().enumerate() {
                *c = self.estimate_slot(slot, n, bps);
            }
            let best = pick(&cost);
            for slot in modes[best].1 {
                self.analyze_slot(slot, n, bps);
            }
            if best != 0 {
                // The estimate chose; exact bits decide against plain coding.
                let [a, b] = modes[best].1;
                let total = self.slots[a].bits + self.slots[b].bits;
                let plain: u64 = (0..2)
                    .map(|c| {
                        if is_constant(&self.planes[c][..n]) {
                            8 + u64::from(bps)
                        } else {
                            8 + n as u64 * u64::from(bps)
                        }
                    })
                    .sum();
                if total > plain {
                    for c in 0..2 {
                        self.analyzer
                            .trivial(&self.planes[c][..n], bps, &mut self.slots[c]);
                    }
                    return modes[0].0;
                }
            }
            best
        };
        modes[best].0
    }

    fn write_frame(&mut self, n: usize, number: u64, out: &mut Vec<u8>) {
        let bps = u32::from(self.config.bits_per_sample);
        let channels = usize::from(self.config.channels);
        let assignment = if channels == 2 && self.level.stereo != Stereo::Off {
            self.analyze_stereo(n, bps)
        } else {
            for c in 0..channels {
                self.analyze_slot(c, n, bps);
            }
            ChannelAssignment::Independent(self.config.channels)
        };
        let start = out.len();
        frame::write_header(
            &FrameHeader {
                variable_block_size: false,
                block_size: n as u32,
                sample_rate: Some(self.config.sample_rate),
                channels: assignment,
                bits_per_sample: Some(self.config.bits_per_sample),
                number,
                header_len: 0,
            },
            out,
        );
        let mut sink = BitSink::new(out);
        let slots: [usize; 8] = match assignment {
            ChannelAssignment::Independent(_) => [0, 1, 2, 3, 4, 5, 6, 7],
            ChannelAssignment::LeftSide => [0, 3, 0, 0, 0, 0, 0, 0],
            ChannelAssignment::SideRight => [3, 1, 0, 0, 0, 0, 0, 0],
            ChannelAssignment::MidSide => [2, 3, 0, 0, 0, 0, 0, 0],
        };
        for &slot in &slots[..usize::from(assignment.channels())] {
            let plan = &self.slots[slot];
            if slot < channels {
                write::write_subframe(&mut sink, plan, &self.planes[slot][..n]);
            } else if slot == 2 {
                write::write_subframe(&mut sink, plan, &self.mid[..n]);
            } else if bps < 32 {
                write::write_subframe(&mut sink, plan, &self.side32[..n]);
            } else {
                write::write_subframe(&mut sink, plan, &self.side64[..n]);
            }
        }
        sink.finish();
        let crc = crate::crc::crc16(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }

    #[cfg(test)]
    fn force_stereo(&mut self, mode: ChannelAssignment) {
        self.force = Some(mode);
    }
}

#[cfg(test)]
mod tests;
