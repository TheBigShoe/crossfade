//! FLAC decoder adapter: one packet (one whole frame) in, one block of samples out, left-justified
//! in the stream's container format. The decoding itself lives in `crossfade-flac`. With
//! threads, frames come out a few packets late (each with its own packet's pts), the rest at the
//! drain; the frames and the errors are the same at any thread count.

use crossfade_core::{
    AudioFrame, ChannelLayout, Decoder, Error, Packet, SampleFormat, Samples, StreamInfo,
};
use crossfade_flac::Block;
use crossfade_flac::streaminfo::StreamInfoBlock;
use std::collections::VecDeque;

/// The same texts the engine had before the decoder moved out: the standalone crate's
/// messages already carry their `flac:` prefix.
pub(crate) fn engine_error(e: crossfade_flac::Error) -> Error {
    match e {
        crossfade_flac::Error::Malformed(what) => Error::Malformed { what },
        crossfade_flac::Error::Unsupported(what) => Error::Unsupported { what },
        crossfade_flac::Error::UnexpectedEof { offset } => Error::UnexpectedEof {
            offset: offset as u64,
        },
        // The engine has no variant for a bug; the text says what it was.
        other => Error::Unsupported {
            what: other.to_string(),
        },
    }
}

fn format_for(bits: u8) -> SampleFormat {
    match bits {
        0..=8 => SampleFormat::U8,
        9..=16 => SampleFormat::S16,
        17..=24 => SampleFormat::S24,
        _ => SampleFormat::S32,
    }
}

struct FlacDecoder {
    inner: crossfade_flac::Decoder,
    shape: Shape,
    /// Pts of the packets whose blocks have not come out yet, oldest first.
    pts: VecDeque<i64>,
    /// Decoded blocks waiting for `receive_frame`, oldest first.
    ready: VecDeque<AudioFrame>,
    /// Frames to decode into; `receive_frame` trades a ready one for the caller's, so a few
    /// buffers keep being reused.
    spare: Vec<AudioFrame>,
    draining: bool,
    /// A frame failed; the stream is over and its end is not checked.
    failed: bool,
    /// The failure, held back until the frames decoded before it have been received.
    held: Option<Error>,
    end_checked: bool,
}

/// What every frame this decoder hands out looks like.
struct Shape {
    format: SampleFormat,
    layout: ChannelLayout,
    sample_rate: u32,
    template: AudioFrame,
}

impl Shape {
    fn fill(&self, block: Block<'_>, pts: i64, f: &mut AudioFrame) {
        let bits = u32::from(block.bits_per_sample());
        f.pts = pts;
        f.sample_rate = self.sample_rate;
        if f.layout != self.layout {
            f.layout = self.layout.clone();
        }
        if f.samples.format() != self.format {
            f.samples = Samples::new(self.format);
        }
        match &mut f.samples {
            Samples::U8(out) => block.interleave_u8(8 - bits, out),
            Samples::S16(out) => block.interleave_i16(16 - bits, out),
            Samples::S24(out) => block.interleave_i32(24 - bits, out),
            Samples::S32(out) => block.interleave_i32(32 - bits, out),
            Samples::F32(_) | Samples::F64(_) => unreachable!("format_for never picks floats"),
        }
    }
}

pub fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
    decoder_threads(stream, 1)
}

/// As [`decoder`], decoding on up to `threads` threads (the calling thread included; 0 means
/// the machine's parallelism); see `crossfade_flac::Decoder::with_threads`.
pub fn decoder_threads(stream: &StreamInfo, threads: usize) -> Result<Box<dyn Decoder>, Error> {
    let info = StreamInfoBlock::parse(&stream.extradata).map_err(engine_error)?;
    let format = format_for(info.bits_per_sample);
    if stream.audio.format() != format
        || stream.audio.layout().channels() != u16::from(info.channels)
        || stream.audio.sample_rate() != info.sample_rate
    {
        return Err(Error::Malformed {
            what: "flac: stream description disagrees with its STREAMINFO".into(),
        });
    }
    Ok(Box::new(FlacDecoder {
        inner: crossfade_flac::Decoder::with_threads(&info, threads).map_err(engine_error)?,
        shape: Shape {
            format,
            layout: stream.audio.layout().clone(),
            sample_rate: info.sample_rate,
            template: AudioFrame::new(&stream.audio),
        },
        pts: VecDeque::new(),
        ready: VecDeque::new(),
        spare: Vec::new(),
        draining: false,
        failed: false,
        held: None,
        end_checked: false,
    }))
}

impl FlacDecoder {
    /// Runs `f` with a callback that turns each delivered block into a ready frame.
    fn collect(
        &mut self,
        f: impl FnOnce(
            &mut crossfade_flac::Decoder,
            &mut dyn FnMut(Block<'_>),
        ) -> crossfade_flac::Result<()>,
    ) -> Result<(), Error> {
        let Self {
            inner,
            shape,
            pts,
            ready,
            spare,
            ..
        } = self;
        let mut on_block = |block: Block<'_>| {
            let mut frame = spare.pop().unwrap_or_else(|| shape.template.clone());
            shape.fill(block, pts.pop_front().unwrap_or_default(), &mut frame);
            ready.push_back(frame);
        };
        let result = f(inner, &mut on_block).map_err(engine_error);
        match result {
            // Frames from before the failure come out first, as they would one at a time.
            Err(e) if !self.ready.is_empty() => {
                self.failed = true;
                self.held = Some(e);
                Ok(())
            }
            Err(e) => {
                self.failed = true;
                Err(e)
            }
            Ok(()) => Ok(()),
        }
    }
}

impl Decoder for FlacDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        self.pts.push_back(packet.pts);
        let data = &packet.data;
        self.collect(|inner, on_block| inner.push_frame(data, on_block))
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if let Some(mut frame) = self.ready.pop_front() {
            std::mem::swap(out, &mut frame);
            self.spare.push(frame);
            return Ok(true);
        }
        if let Some(e) = self.held.take() {
            return Err(e);
        }
        if self.draining && !self.end_checked && !self.failed {
            self.end_checked = true;
            self.collect(|inner, on_block| inner.flush(on_block))?;
            if let Some(mut frame) = self.ready.pop_front() {
                // The rest of the flushed frames come out on the following calls; the end is
                // checked once they have (`finish` needs nothing in flight, and nothing is).
                self.end_checked = false;
                std::mem::swap(out, &mut frame);
                self.spare.push(frame);
                return Ok(true);
            }
            self.inner.finish().map_err(engine_error)?;
        }
        Ok(false)
    }

    fn drain(&mut self) {
        self.draining = true;
    }

    fn flush(&mut self) -> Result<(), Error> {
        if self.failed {
            return self.held.clone().map_or(Ok(()), Err);
        }
        self.collect(|inner, on_block| inner.flush(on_block))
    }

    /// Also turns off the whole-stream MD5 and total checks: after a seek they cannot hold.
    fn reset(&mut self) {
        self.inner.reset();
        self.pts.clear();
        self.spare.extend(self.ready.drain(..));
        self.draining = false;
        self.failed = false;
        self.held = None;
        self.end_checked = false;
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
