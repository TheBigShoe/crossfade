//! The FLAC decoder (RFC 9639): one whole frame in, one block of raw samples out, with the
//! stream's MD5 and sample total checked once the last frame is in.
//!
//! The decoder knows nothing of containers: whatever splits the stream into frames (a FLAC
//! or Ogg reader) hands each frame over as a slice.

use crate::crc::{crc16, crc16_update};
use crate::error::{Error, Result};
use crate::frame::{self, ChannelAssignment, FrameHeader, HeaderError};
use crate::md5::Md5;
use crate::pool::{Ordered, Panicked, Serial};
use crate::streaminfo::StreamInfoBlock;
use crate::subframe::{Bits, read_subframe};

fn malformed(what: &str) -> Error {
    Error::Malformed(format!("flac: {what}"))
}

/// What [`Decoder::finish`] could and could not check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verification {
    /// The hash of the decoded samples equals the one in STREAMINFO.
    Md5Matched,
    /// STREAMINFO carries no hash (all zero); only the sample total, if it has one, was checked.
    NoMd5,
    /// A frame failed earlier, so the total and the hash say nothing about the stream.
    Skipped,
}

/// Decodes the frames of one stream, one after another.
pub struct Decoder {
    info: StreamInfoBlock,
    frames: FrameDecoder,
    /// The finished block, one buffer per channel, reused from frame to frame.
    planes: Vec<Vec<i32>>,
    md5: Md5,
    md5_bytes: Vec<u8>,
    /// Samples per channel decoded before the current block.
    first: u64,
    decoded: u64,
    block: usize,
    /// A frame failed: the totals and the hash no longer say anything about the stream.
    failed: bool,
    finished: bool,
    /// The error that ended a stream given through `push_frame`; later calls repeat it.
    error: Option<Error>,
    /// Resolved thread count; 1 decodes every frame on the calling thread.
    threads: usize,
    /// Made by the first frame `push_frame` hands over when `threads > 1`.
    pool: Option<Ordered<FrameJob, FrameJob, FrameDecoder>>,
    /// The MD5 on a thread of its own once workers run, fed each frame's packed samples in
    /// order as its block is delivered.
    md5_stage: Option<Serial<Md5, Vec<u8>>>,
    spare_jobs: Vec<FrameJob>,
    /// Frames given to `push_frame`, for the jobs' numbers.
    pushed: u64,
    /// Makes the decoding of pushed frame N panic, for the failure tests.
    #[cfg(test)]
    panic_at: Option<u64>,
}

/// One frame on its way through the pool: a copy of its bytes in; its block, its samples
/// packed for the MD5 and its outcome out. The buffers are reused from frame to frame.
struct FrameJob {
    number: u64,
    data: Vec<u8>,
    planes: Vec<Vec<i32>>,
    md5_bytes: Vec<u8>,
    block: usize,
    result: Result<()>,
}

/// A worker decodes the frame, checks its CRC-16 and packs the MD5's bytes, which leaves the
/// MD5 thread, the one stage that cannot be split, nothing but hashing. Errors come out as the
/// one-frame-at-a-time decoder reports them, the CRC's taking precedence the same way.
fn decode_job(frames: &mut FrameDecoder, mut job: FrameJob) -> FrameJob {
    #[cfg(test)]
    if frames.panic_at == Some(job.number) {
        panic!("test panic at frame {}", job.number);
    }
    let decoded = frames.decode(&job.data, &mut job.planes);
    job.result = decoded.and_then(|block| {
        job.block = block;
        if frames.info.md5 != [0; 16] {
            pack_md5(
                frames.info.bits_per_sample,
                &job.planes,
                block,
                &mut job.md5_bytes,
            );
        }
        if crc16(&job.data) == 0 {
            Ok(())
        } else {
            Err(crc_mismatch())
        }
    });
    job
}

/// Blocks in flight per thread that decodes: enough that a worker always finds the next frame
/// waiting, few enough that memory stays at a handful of blocks.
const DEPTH: usize = 2;
/// Frames waiting with no worker free before one starts.
const SPAWN_THRESHOLD: usize = 4;
/// Blocks' bytes waiting for the MD5 thread before the caller waits for it.
const MD5_DEPTH: usize = 8;

fn panicked(p: Panicked) -> Error {
    Error::Internal(format!(
        "flac decoder: a worker thread panicked ({}); the stream cannot continue",
        p.0
    ))
}

impl Decoder {
    /// `threads` value that means "as many as the machine runs at once"
    /// ([`auto_thread_count`](crate::auto_thread_count)).
    pub const AUTO_THREADS: usize = 0;
    /// The most threads a decoder uses, whatever `threads` says.
    pub const MAX_THREADS: usize = 64;

    /// `info` must describe 1 to 8 channels of 4 to 32 bits at a non-zero rate, as
    /// [`StreamInfoBlock::parse`] guarantees. Decodes on the calling thread only.
    pub fn new(info: &StreamInfoBlock) -> Result<Self> {
        Self::with_threads(info, 1)
    }

    /// As [`new`](Self::new), letting [`push_frame`](Self::push_frame) decode on up to
    /// `threads` threads, the calling thread included; the results never depend on it. `1`
    /// decodes on the calling thread and starts no thread at all. More hand frames to up to
    /// `threads - 1` workers, started only as frames queue up, and hash the audio on one more;
    /// blocks then come out a few frames later, in order. [`AUTO_THREADS`](Self::AUTO_THREADS)
    /// (0) takes [`auto_thread_count`](crate::auto_thread_count); counts above
    /// [`MAX_THREADS`](Self::MAX_THREADS) count as that.
    pub fn with_threads(info: &StreamInfoBlock, threads: usize) -> Result<Self> {
        if !(1..=8).contains(&info.channels) {
            return Err(malformed("channel count outside 1 to 8"));
        }
        if !(4..=32).contains(&info.bits_per_sample) {
            return Err(malformed("bit depth outside 4 to 32"));
        }
        if info.sample_rate == 0 {
            return Err(malformed("sample rate is zero"));
        }
        let channels = usize::from(info.channels);
        Ok(Self {
            info: *info,
            frames: FrameDecoder::new(info),
            planes: vec![Vec::new(); channels],
            md5: Md5::new(),
            md5_bytes: Vec::new(),
            first: 0,
            decoded: 0,
            block: 0,
            failed: false,
            finished: false,
            error: None,
            threads: match threads {
                Self::AUTO_THREADS => crate::auto_thread_count(),
                n => n.min(Self::MAX_THREADS),
            },
            pool: None,
            md5_stage: None,
            spare_jobs: Vec::new(),
            pushed: 0,
            #[cfg(test)]
            panic_at: None,
        })
    }

    /// Decodes `frame` (as for [`decode_frame`](Self::decode_frame)) and hands every block
    /// that is ready, in stream order, to `on_block`. With one thread that is this frame's
    /// block, at once; with more it may be earlier frames' blocks, a few frames late, and
    /// [`flush`](Self::flush) hands over the rest. At most `2 * threads` frames are in flight;
    /// their bytes are copied into reused buffers.
    ///
    /// When a frame fails, every block before it has been handed over and then its error is
    /// returned (from this call or a later one); it is the error `decode_frame` gives for that
    /// frame. The stream is then over: no later block is handed over, every later call returns
    /// the same error, and [`finish`](Self::finish) reports `Skipped`.
    pub fn push_frame(&mut self, frame: &[u8], on_block: &mut dyn FnMut(Block<'_>)) -> Result<()> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        if self.threads <= 1 {
            if let Err(e) = self.decode(frame) {
                self.failed = true;
                self.error = Some(e.clone());
                return Err(e);
            }
            on_block(self.current());
            return Ok(());
        }
        let mut job = self.spare_jobs.pop().unwrap_or_else(|| FrameJob {
            number: 0,
            data: Vec::new(),
            planes: vec![Vec::new(); usize::from(self.info.channels)],
            md5_bytes: Vec::new(),
            block: 0,
            result: Ok(()),
        });
        job.number = self.pushed;
        job.data.clear();
        job.data.extend_from_slice(frame);
        self.pushed += 1;
        if self.pool.is_none() {
            let info = self.info;
            #[cfg(test)]
            let panic_at = self.panic_at;
            #[cfg(test)]
            {
                self.frames.panic_at = panic_at;
            }
            self.pool = Some(
                Ordered::new(
                    self.threads - 1,
                    DEPTH,
                    move || {
                        #[allow(unused_mut)]
                        let mut frames = FrameDecoder::new(&info);
                        #[cfg(test)]
                        {
                            frames.panic_at = panic_at;
                        }
                        frames
                    },
                    decode_job,
                )
                // A frame decodes in a fraction of the time a thread takes to start, so a stream
                // of a few frames is left to the caller.
                .spawn_threshold(SPAWN_THRESHOLD),
            );
        }
        while self.pool.as_ref().is_some_and(Ordered::is_full) {
            self.take_one(on_block)?;
        }
        let Some(pool) = &mut self.pool else {
            unreachable!("made above")
        };
        pool.submit(job);
        if pool.workers() > 0 && self.md5_stage.is_none() && self.info.md5 != [0; 16] {
            // Workers are running, so the stream is long enough for MD5 to pay for a thread.
            let md5 = std::mem::take(&mut self.md5);
            match Serial::new(md5, hash_bytes, MD5_DEPTH) {
                Ok(stage) => self.md5_stage = Some(stage),
                Err(md5) => self.md5 = md5,
            }
        }
        self.take_ready(on_block)
    }

    /// Hands over every block still in flight, in order (see [`push_frame`](Self::push_frame)
    /// for errors). Does nothing when none are.
    pub fn flush(&mut self, on_block: &mut dyn FnMut(Block<'_>)) -> Result<()> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        while self.frames_in_flight() > 0 {
            self.take_one(on_block)?;
        }
        Ok(())
    }

    /// Frames given to [`push_frame`](Self::push_frame) whose blocks have not been handed
    /// over: always 0 with one thread, at most `2 * threads` with more.
    pub fn frames_in_flight(&self) -> usize {
        self.pool.as_ref().map_or(0, Ordered::in_flight)
    }

    /// Threads started and still running (decoding workers and the MD5 thread).
    #[cfg(test)]
    pub(crate) fn workers(&self) -> usize {
        self.pool.as_ref().map_or(0, Ordered::workers) + usize::from(self.md5_stage.is_some())
    }

    fn current(&self) -> Block<'_> {
        Block {
            planes: &self.planes,
            len: self.block,
            bits: self.info.bits_per_sample,
            first: self.first,
        }
    }

    /// Ends the stream on the first error: the threads stop and later calls repeat it.
    fn fail(&mut self, e: Error) -> Error {
        self.failed = true;
        self.error = Some(e.clone());
        // Dropping them stops and joins the threads; the frames after this one are discarded.
        self.pool = None;
        self.md5_stage = None;
        e
    }

    fn take_ready(&mut self, on_block: &mut dyn FnMut(Block<'_>)) -> Result<()> {
        while let Some(result) = self.pool.as_mut().and_then(Ordered::try_next) {
            self.deliver(result, on_block)?;
        }
        Ok(())
    }

    /// Waits for the next block in order (decoding queued frames meanwhile) and hands it over.
    fn take_one(&mut self, on_block: &mut dyn FnMut(Block<'_>)) -> Result<()> {
        let next = match &mut self.pool {
            Some(pool) => pool.next(&mut self.frames),
            None => None,
        };
        match next {
            Some(result) => self.deliver(result, on_block),
            None => Ok(()),
        }
    }

    fn deliver(
        &mut self,
        result: std::result::Result<FrameJob, Panicked>,
        on_block: &mut dyn FnMut(Block<'_>),
    ) -> Result<()> {
        let mut job = match result {
            Ok(job) => job,
            Err(p) => return Err(self.fail(panicked(p))),
        };
        if let Err(e) = std::mem::replace(&mut job.result, Ok(())) {
            return Err(self.fail(e));
        }
        if self.info.md5 != [0; 16] {
            if let Some(stage) = &mut self.md5_stage {
                let spare = stage.spare().unwrap_or_default();
                let sent = stage.send(std::mem::replace(&mut job.md5_bytes, spare));
                if let Err(p) = sent {
                    return Err(self.fail(panicked(p)));
                }
            } else {
                self.md5.update(&job.md5_bytes);
            }
        }
        self.first = self.decoded;
        self.decoded += job.block as u64;
        self.block = job.block;
        on_block(Block {
            planes: &job.planes,
            len: job.block,
            bits: self.info.bits_per_sample,
            first: self.first,
        });
        self.spare_jobs.push(job);
        Ok(())
    }

    /// Brings the MD5 back from its thread, which has every block's bytes once nothing is in
    /// flight, so that `decode_frame` and `finish` can carry on with it.
    fn reclaim_md5(&mut self) -> Result<()> {
        if let Some(stage) = self.md5_stage.take() {
            match stage.finish() {
                Ok(md5) => self.md5 = md5,
                Err(p) => return Err(self.fail(panicked(p))),
            }
        }
        Ok(())
    }

    /// Decodes one whole frame (as split by a container reader), checking its CRC-16. Zero
    /// bytes after the frame are tolerated: a reader cannot tell them from the frame's end.
    ///
    /// The block borrows the decoder, so it is gone before the next frame is decoded. After an
    /// error the decoder still takes frames, but [`finish`](Self::finish) reports `Skipped`.
    pub fn decode_frame(&mut self, frame: &[u8]) -> Result<Block<'_>> {
        if self.frames_in_flight() > 0 {
            return Err(malformed(
                "frames from push_frame() are in flight; flush() first",
            ));
        }
        self.reclaim_md5()?;
        if let Err(e) = self.decode(frame) {
            self.failed = true;
            return Err(e);
        }
        Ok(self.current())
    }

    /// Call after the last frame: checks the sample total and the MD5 against STREAMINFO, when
    /// it has them. A mismatch, or a stream shorter or longer than announced, is an error.
    /// With frames from [`push_frame`](Self::push_frame) still in flight it refuses (their
    /// blocks would be lost unseen): [`flush`](Self::flush) first. It joins any threads.
    pub fn finish(&mut self) -> Result<Verification> {
        if self.frames_in_flight() > 0 {
            return Err(malformed("finish() with frames in flight; flush() first"));
        }
        self.pool = None;
        if !self.failed {
            self.reclaim_md5()?;
        }
        if self.failed || std::mem::replace(&mut self.finished, true) {
            return Ok(Verification::Skipped);
        }
        let total = self.info.total_samples;
        if total != 0 && total != self.decoded {
            return Err(malformed(&format!(
                "stream ended after {} of {total} samples",
                self.decoded
            )));
        }
        if self.info.md5 == [0; 16] {
            return Ok(Verification::NoMd5);
        }
        if std::mem::take(&mut self.md5).finalize() != self.info.md5 {
            return Err(malformed(
                "decoded audio does not match the MD5 in the stream header",
            ));
        }
        Ok(Verification::Md5Matched)
    }

    /// The frame CRC-16 is computed last, alongside the MD5 (see `hash`), but it still decides:
    /// a frame whose CRC fails reports that, whatever else is wrong with it.
    fn decode(&mut self, data: &[u8]) -> Result<()> {
        let block = self.frames.decode(data, &mut self.planes)?;
        if self.hash(block, data) != 0 {
            return Err(crc_mismatch());
        }
        self.first = self.decoded;
        self.decoded += block as u64;
        self.block = block;
        Ok(())
    }

    /// Hashes the raw samples as the format defines it: interleaved, little-endian, in the
    /// smallest whole number of bytes the depth fits. Skipped when there is nothing to compare
    /// the hash with. Returns the CRC-16 of `frame`, computed between the MD5's blocks: both are
    /// serial chains, and interleaved the CRC's runs while the MD5 waits on its own.
    fn hash(&mut self, block: usize, frame: &[u8]) -> u16 {
        if self.info.md5 == [0; 16] {
            return crc16(frame);
        }
        pack_md5(
            self.info.bits_per_sample,
            &self.planes,
            block,
            &mut self.md5_bytes,
        );
        let (mut crc, mut done) = (0, 0);
        self.md5.update_interleaved(&self.md5_bytes, |blocks_left| {
            let rest = &frame[done..];
            // An even share of what is left, in whole steps of the CRC.
            let step = rest
                .len()
                .div_ceil(blocks_left + 1)
                .next_multiple_of(16)
                .min(rest.len());
            crc = crc16_update(crc, &rest[..step]);
            done += step;
        });
        crc16_update(crc, &frame[done..])
    }
}

/// What decoding one frame needs beside its bytes and its output: the stream's parameters and,
/// for a 32-bit stream, wide work buffers. Nothing in it carries over from one frame to the
/// next, so frames can be decoded by any number of these in any order.
pub(crate) struct FrameDecoder {
    info: StreamInfoBlock,
    /// The stream is too deep for `i32` subframes (see `work`).
    wide: bool,
    /// Work buffers for 32-bit streams only, whose side channel needs 33 bits; every other
    /// stream decodes straight into the planes.
    work: Vec<Vec<i64>>,
    #[cfg(test)]
    panic_at: Option<u64>,
}

impl FrameDecoder {
    pub(crate) fn new(info: &StreamInfoBlock) -> Self {
        Self {
            info: *info,
            wide: info.bits_per_sample > 31,
            work: vec![Vec::new(); usize::from(info.channels)],
            #[cfg(test)]
            panic_at: None,
        }
    }

    fn check_parameters(&self, h: &FrameHeader) -> Result<()> {
        let unsupported = |what: &str, was: u32, now: u32| {
            Error::Unsupported(format!(
                "flac: {what} changed from {was} to {now} mid-stream"
            ))
        };
        let channels = h.channels.channels();
        if channels != self.info.channels {
            return Err(unsupported(
                "channel count",
                u32::from(self.info.channels),
                u32::from(channels),
            ));
        }
        let bits = h.bits_per_sample.unwrap_or(self.info.bits_per_sample);
        if bits != self.info.bits_per_sample {
            return Err(unsupported(
                "bit depth",
                u32::from(self.info.bits_per_sample),
                u32::from(bits),
            ));
        }
        let rate = h.sample_rate.unwrap_or(self.info.sample_rate);
        if rate != self.info.sample_rate {
            return Err(unsupported("sample rate", self.info.sample_rate, rate));
        }
        Ok(())
    }

    /// Everything in a frame but the CRC-16 over the whole of it, which the caller checks
    /// (with the MD5 when it can): the header, the stream's parameters, the subframes and the
    /// decorrelation, into `planes`. Returns the block size. Every error but the CRC's is
    /// reported here, and one the CRC would explain is reported as a CRC mismatch.
    fn decode(&mut self, data: &[u8], planes: &mut [Vec<i32>]) -> Result<usize> {
        let h = frame::parse_header(data).map_err(|e| match e {
            HeaderError::NeedMoreData => malformed("frame header cut short"),
            HeaderError::Invalid(why) => malformed(why),
        })?;
        self.check_parameters(&h)?;
        if data.len() < h.header_len + 2 {
            return Err(crc_mismatch());
        }
        let block = h.block_size as usize;
        let end = match self.subframes(&h, data, planes) {
            Ok(end) => end,
            Err(_) if crc16(data) != 0 => return Err(crc_mismatch()),
            Err(e) => return Err(e),
        };
        // The CRC-16 follows the subframes. Zero bytes after it are padding the container
        // reader could not tell from the frame (the CRC-16 stays zero over zeros).
        if end + 2 < data.len() {
            if crc16(data) != 0 || crc16(&data[..end + 2]) != 0 {
                return Err(crc_mismatch());
            }
            if data[end + 2..].iter().any(|&b| b != 0) {
                return Err(malformed(
                    "unused bytes between the subframes and the CRC-16",
                ));
            }
        }
        if self.wide {
            self.decorrelate(h.channels);
            self.narrow(block, planes);
        } else {
            decorrelate_i32(planes, h.channels);
        }
        Ok(block)
    }

    /// Decodes every subframe; returns where they end in `data`.
    fn subframes(
        &mut self,
        h: &FrameHeader,
        data: &[u8],
        planes: &mut [Vec<i32>],
    ) -> Result<usize> {
        let block = h.block_size as usize;
        let bps = u32::from(self.info.bits_per_sample);
        let mut r = Bits::new(&data[h.header_len..data.len() - 2]);
        // The side channel of a stereo pair has one more bit.
        let side = match h.channels {
            ChannelAssignment::LeftSide | ChannelAssignment::MidSide => Some(1),
            ChannelAssignment::SideRight => Some(0),
            ChannelAssignment::Independent(_) => None,
        };
        let side_channel = |c: usize| bps + u32::from(side == Some(c));
        if self.wide {
            for (c, out) in self.work.iter_mut().enumerate() {
                read_subframe(&mut r, out, block, side_channel(c))?;
            }
        } else {
            for (c, out) in planes.iter_mut().enumerate() {
                read_subframe(&mut r, out, block, side_channel(c))?;
            }
        }
        r.align();
        Ok(data.len() - 2 - (r.bits_left() / 8) as usize)
    }

    fn decorrelate(&mut self, assignment: ChannelAssignment) {
        if matches!(assignment, ChannelAssignment::Independent(_)) {
            return;
        }
        // The other assignments are stereo pairs, so two buffers exist.
        let (first, rest) = self.work.split_at_mut(1);
        let (a, b) = (&mut first[0], &mut rest[0]);
        match assignment {
            ChannelAssignment::Independent(_) => {}
            ChannelAssignment::LeftSide => {
                for (l, s) in a.iter().zip(b.iter_mut()) {
                    *s = l.wrapping_sub(*s);
                }
            }
            ChannelAssignment::SideRight => {
                for (s, r) in a.iter_mut().zip(b.iter()) {
                    *s = s.wrapping_add(*r);
                }
            }
            ChannelAssignment::MidSide => {
                for (m, s) in a.iter_mut().zip(b.iter_mut()) {
                    // The mid channel dropped the low bit of L + R; the side's parity restores it.
                    let sum = (m.wrapping_shl(1)) | (*s & 1);
                    *m = sum.wrapping_add(*s) >> 1;
                    *s = sum.wrapping_sub(*s) >> 1;
                }
            }
        }
    }

    /// Truncates the decorrelated channels to `i32`: exact for a valid stream, and the same
    /// low bits as before for a corrupt one that got past the CRC.
    fn narrow(&self, block: usize, planes: &mut [Vec<i32>]) {
        for (plane, work) in planes.iter_mut().zip(&self.work) {
            plane.clear();
            plane.reserve(block);
            plane.extend(work.iter().map(|&v| v as i32));
        }
    }
}

/// The block's samples as the MD5 takes them.
fn pack_md5(bits_per_sample: u8, planes: &[Vec<i32>], block: usize, bytes: &mut Vec<u8>) {
    match bits_per_sample.div_ceil(8) {
        1 => pack::<1>(planes, block, bytes),
        2 => pack::<2>(planes, block, bytes),
        3 => pack::<3>(planes, block, bytes),
        _ => pack::<4>(planes, block, bytes),
    }
}

#[allow(clippy::ptr_arg)] // the serial stage passes a reference to its item type
fn hash_bytes(md5: &mut Md5, bytes: &Vec<u8>) {
    md5.update(bytes);
}

fn crc_mismatch() -> Error {
    malformed("frame CRC-16 mismatch")
}

/// The planes as interleaved little-endian samples of `W` bytes each. With the width a constant
/// each sample is a plain store, not a `memcpy` call.
// `as_chunks_mut::<{ 2 * W }>` needs unstable const expressions.
#[allow(clippy::chunks_exact_to_as_chunks)]
fn pack<const W: usize>(planes: &[Vec<i32>], block: usize, bytes: &mut Vec<u8>) {
    let nch = planes.len();
    // Every byte is overwritten below, so a buffer of the right size is reused as it is.
    bytes.resize(block * nch * W, 0);
    if let [left, right] = planes {
        for ((slot, &l), &r) in bytes.chunks_exact_mut(2 * W).zip(left).zip(right) {
            slot[..W].copy_from_slice(&l.to_le_bytes()[..W]);
            slot[W..].copy_from_slice(&r.to_le_bytes()[..W]);
        }
        return;
    }
    pack_any::<W>(planes, bytes);
}

fn pack_any<const W: usize>(planes: &[Vec<i32>], bytes: &mut [u8]) {
    let nch = planes.len();
    for (c, plane) in planes.iter().enumerate() {
        let slots = bytes.as_chunks_mut::<W>().0.iter_mut().skip(c).step_by(nch);
        for (slot, &v) in slots.zip(plane) {
            slot.copy_from_slice(&v.to_le_bytes()[..W]);
        }
    }
}

/// Turns the channels of a stereo pair back into left and right, in place.
fn decorrelate_i32(planes: &mut [Vec<i32>], assignment: ChannelAssignment) {
    if matches!(assignment, ChannelAssignment::Independent(_)) {
        return;
    }
    // The other assignments are stereo pairs, so two buffers exist.
    let (first, rest) = planes.split_at_mut(1);
    let (a, b) = (&mut first[0], &mut rest[0]);
    match assignment {
        ChannelAssignment::Independent(_) => {}
        ChannelAssignment::LeftSide => {
            for (l, s) in a.iter().zip(b.iter_mut()) {
                *s = l.wrapping_sub(*s);
            }
        }
        ChannelAssignment::SideRight => {
            for (s, r) in a.iter_mut().zip(b.iter()) {
                *s = s.wrapping_add(*r);
            }
        }
        ChannelAssignment::MidSide => {
            for (m, s) in a.iter_mut().zip(b.iter_mut()) {
                // The mid channel dropped the low bit of L + R; the side's parity restores it.
                let sum = m.wrapping_shl(1) | (*s & 1);
                *m = sum.wrapping_add(*s) >> 1;
                *s = sum.wrapping_sub(*s) >> 1;
            }
        }
    }
}

/// One decoded frame, borrowed from the decoder: raw sign-extended samples per channel.
#[derive(Debug, Clone, Copy)]
pub struct Block<'a> {
    planes: &'a [Vec<i32>],
    len: usize,
    bits: u8,
    first: u64,
}

/// Writes the planes interleaved into `out` through `f`.
fn interleave<T: Copy + Default>(
    planes: &[Vec<i32>],
    len: usize,
    out: &mut Vec<T>,
    f: impl Fn(i32) -> T,
) {
    let nch = planes.len();
    // Every slot is overwritten below, so a buffer of the right size is reused as it is.
    out.resize(len * nch, T::default());
    match planes {
        [mono] => {
            for (dst, &v) in out.iter_mut().zip(mono) {
                *dst = f(v);
            }
        }
        [left, right] => {
            for ((dst, &l), &r) in out.as_chunks_mut::<2>().0.iter_mut().zip(left).zip(right) {
                dst[0] = f(l);
                dst[1] = f(r);
            }
        }
        _ => interleave_any(planes, out, f),
    }
}

fn interleave_any<T: Copy>(planes: &[Vec<i32>], out: &mut [T], f: impl Fn(i32) -> T) {
    let nch = planes.len();
    for (c, plane) in planes.iter().enumerate() {
        for (dst, &v) in out.iter_mut().skip(c).step_by(nch).zip(plane) {
            *dst = f(v);
        }
    }
}

impl Block<'_> {
    pub fn channels(&self) -> usize {
        self.planes.len()
    }

    /// Samples per channel.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn bits_per_sample(&self) -> u8 {
        self.bits
    }

    /// Index, per channel, of this block's first sample in the stream: the samples decoded
    /// before it.
    pub fn first_sample(&self) -> u64 {
        self.first
    }

    /// The raw sample values of one channel, sign-extended from `bits_per_sample`, not
    /// left-justified.
    ///
    /// # Panics
    ///
    /// If `index` is not below [`channels`](Self::channels).
    pub fn channel(&self, index: usize) -> &[i32] {
        &self.planes[index]
    }

    /// Interleaves into `out`, replacing its contents, each sample shifted left by `shift`
    /// (below 32) to fill a 32-bit container.
    pub fn interleave_i32(&self, shift: u32, out: &mut Vec<i32>) {
        interleave(self.planes, self.len, out, |v| v << shift);
    }

    /// As [`interleave_i32`](Self::interleave_i32) into 16-bit samples; `shift` is
    /// `16 - bits_per_sample` for a stream that fits.
    pub fn interleave_i16(&self, shift: u32, out: &mut Vec<i16>) {
        interleave(self.planes, self.len, out, |v| (v << shift) as i16);
    }

    /// As [`interleave_i32`](Self::interleave_i32) into unsigned 8-bit samples: the shifted
    /// value plus 128, so 8-bit audio keeps the usual offset binary form.
    pub fn interleave_u8(&self, shift: u32, out: &mut Vec<u8>) {
        interleave(self.planes, self.len, out, |v| {
            (v << shift).wrapping_add(128) as u8
        });
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
