//! One input open for decoding, frame by frame, with exact seeking: what a player and the
//! analysis passes read through.

use crate::timeline::Timeline;
use crossfade_codec::decoder_for_threads;
use crossfade_core::{AudioFrame, Decoder, Demuxer, Error, JobIo, Limits, Packet, StreamInfo};
use crossfade_format::open_demuxer;

/// Decodes the one audio stream of an input in order, on its timeline (see [`Timeline`]), and
/// jumps to any frame exactly. Frames come in the stream's own sample format, with pts counting
/// frames from 0. The decoder may use up to `Limits::max_threads` threads; what comes out never
/// depends on it.
pub struct DecodeSession {
    demuxer: Box<dyn Demuxer>,
    decoder: Box<dyn Decoder>,
    stream: StreamInfo,
    timeline: Timeline,
    /// Drops what lies before the target of the last seek (frames shifted to count from it).
    after_seek: Timeline,
    skip_to: u64,
    packet: Packet,
    position: u64,
    /// Where the decoded frames reach since the last seek (or the reader landed): the end, for
    /// a stream that does not know its length, once the input runs out.
    reached: u64,
    /// The reader has ended and the decoder has been told.
    drained: bool,
    /// A reader error held back until the frames decoded before it are out.
    failed: Option<Error>,
}

impl DecodeSession {
    /// Opens `path` through `io`, as `decode_frames` does.
    pub fn open(path: &str, io: &mut dyn JobIo, limits: &Limits) -> Result<Self, Error> {
        let (_, demuxer) = open_demuxer(io.open_source(path)?, limits)?;
        Self::with_demuxer(demuxer, path, limits)
    }

    /// A session over a reader already open; `path` names it in errors.
    pub(crate) fn with_demuxer(
        demuxer: Box<dyn Demuxer>,
        path: &str,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let [stream] = demuxer.streams() else {
            return Err(Error::Unsupported {
                what: format!(
                    "{} streams in {path} (exactly one is supported)",
                    demuxer.streams().len()
                ),
            });
        };
        let stream = stream.clone();
        let decoder = decoder_for_threads(&stream, limits.max_threads)?;
        let unbounded = StreamInfo {
            frames: None,
            ..stream.clone()
        };
        Ok(Self {
            demuxer,
            decoder,
            timeline: Timeline::new(&stream),
            after_seek: Timeline::new(&unbounded),
            skip_to: 0,
            stream,
            packet: Packet::default(),
            position: 0,
            reached: 0,
            drained: false,
            failed: None,
        })
    }

    pub fn stream(&self) -> &StreamInfo {
        &self.stream
    }

    /// The next frames on the timeline, in the stream's own sample format, never empty.
    /// `Ok(false)` at the end. A reader error comes after the frames decoded before it.
    pub fn next_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        loop {
            while self.decoder.receive_frame(out)? {
                let end = out.pts.saturating_add(out.frames() as i64);
                self.reached = self.reached.max(u64::try_from(end).unwrap_or(0));
                if self.keep(out) {
                    self.position = out.pts as u64 + out.frames() as u64;
                    return Ok(true);
                }
            }
            if let Some(e) = self.failed.take() {
                return Err(e);
            }
            if self.drained {
                self.position = self.position.min(self.reached);
                return Ok(false);
            }
            match self.demuxer.read_packet(&mut self.packet) {
                Ok(true) => self.decoder.send_packet(&self.packet)?,
                Ok(false) => {
                    self.decoder.drain();
                    self.drained = true;
                }
                Err(e) => {
                    // A frame read before this packet may hold an earlier error (see `Pipeline`).
                    self.decoder.flush()?;
                    self.failed = Some(e);
                    self.drained = true;
                }
            }
        }
    }

    /// Cuts `frame` to the timeline and to the last seek's target; `false` when nothing is left.
    fn keep(&self, frame: &mut AudioFrame) -> bool {
        if !self.timeline.trim(frame) {
            return false;
        }
        if self.skip_to == 0 {
            return true;
        }
        let shift = i64::try_from(self.skip_to).unwrap_or(i64::MAX);
        frame.pts = frame.pts.saturating_sub(shift);
        let kept = self.after_seek.trim(frame);
        frame.pts = frame.pts.saturating_add(shift);
        kept
    }

    /// Exact: the next frame returned starts at `frame`, clamped to the end (for a stream that
    /// does not know its length, `position` says so once `next_frame` has found the end). The reader lands at or before it and what is decoded before it is
    /// dropped (the frame that straddles it is cut). `Unsupported` when the input cannot seek.
    pub fn seek(&mut self, frame: u64) -> Result<(), Error> {
        let target = self.stream.frames.map_or(frame, |n| frame.min(n));
        let pts = i64::try_from(target).unwrap_or(i64::MAX);
        let landed = self.demuxer.seek(pts)?;
        self.decoder.reset();
        self.skip_to = target;
        self.position = target;
        self.reached = u64::try_from(landed).unwrap_or(0);
        self.drained = false;
        self.failed = None;
        Ok(())
    }

    /// The pts the next frame will carry.
    pub fn position(&self) -> u64 {
        self.position
    }
}
