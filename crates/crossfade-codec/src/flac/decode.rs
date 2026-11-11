//! FLAC decoder adapter: one packet (one whole frame) in, one block of samples out, left-justified
//! in the stream's container format. The decoding itself lives in `crossfade-flac`.

use crossfade_core::{
    AudioFrame, ChannelLayout, Decoder, Error, Packet, SampleFormat, Samples, StreamInfo,
};
use crossfade_flac::streaminfo::StreamInfoBlock;

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
    format: SampleFormat,
    layout: ChannelLayout,
    sample_rate: u32,
    /// The decoded block; `receive_frame` trades it for the caller's frame, so the two
    /// buffers keep being reused.
    frame: AudioFrame,
    ready: bool,
    draining: bool,
    end_checked: bool,
}

pub fn decoder(stream: &StreamInfo) -> Result<Box<dyn Decoder>, Error> {
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
        inner: crossfade_flac::Decoder::new(&info).map_err(engine_error)?,
        format,
        layout: stream.audio.layout().clone(),
        sample_rate: info.sample_rate,
        frame: AudioFrame::new(&stream.audio),
        ready: false,
        draining: false,
        end_checked: false,
    }))
}

impl Decoder for FlacDecoder {
    fn send_packet(&mut self, packet: &Packet) -> Result<(), Error> {
        let block = self
            .inner
            .decode_frame(&packet.data)
            .map_err(engine_error)?;
        let bits = u32::from(block.bits_per_sample());
        let f = &mut self.frame;
        f.pts = packet.pts;
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
        self.ready = true;
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        if self.ready {
            self.ready = false;
            std::mem::swap(out, &mut self.frame);
            return Ok(true);
        }
        if self.draining && !self.end_checked {
            self.end_checked = true;
            self.inner.finish().map_err(engine_error)?;
        }
        Ok(false)
    }

    fn drain(&mut self) {
        self.draining = true;
    }
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
