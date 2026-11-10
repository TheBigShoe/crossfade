use crate::choice::{can_hold, encode_options, output_codec};
use crate::job::{AudioSettings, Job};
use crossfade_codec::{decoder_for, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, Decoder, Demuxer, Encoder, Error, JobIo, Limits, Muxer, Packet,
    SampleFormat, StreamInfo, Tags,
};
use crossfade_dsp::convert::convert_format;
use crossfade_format::{FormatId, create_muxer, open_demuxer};

/// What a job will do, known once the input has been opened.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub input_format: FormatId,
    pub output_format: FormatId,
    pub input: StreamInfo,
    pub output: StreamInfo,
    /// Packets go straight from demuxer to muxer; no decoder or encoder exists.
    pub copy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Progress {
        frames_done: u64,
        frames_total: Option<u64>,
    },
    Finished,
}

struct Transcoder {
    decoder: Box<dyn Decoder>,
    encoder: Box<dyn Encoder>,
    decoded: AudioFrame,
    converted: AudioFrame,
}

enum Engine {
    Copy,
    Transcode(Box<Transcoder>),
}

pub struct Pipeline {
    plan: Plan,
    demuxer: Box<dyn Demuxer>,
    engine: Engine,
    muxer: Box<dyn Muxer>,
    packet: Packet,
    frames_done: u64,
    finished: bool,
}

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

impl Pipeline {
    /// Milestone 1 handles one input and one output, each with one audio
    /// stream. If this fails after the sink exists, the sink is discarded.
    pub fn open(job: &Job, io: &mut dyn JobIo, limits: &Limits) -> Result<Self, Error> {
        if job.version != Job::VERSION {
            return Err(unsupported(format!("job version {}", job.version)));
        }
        let [input] = job.inputs.as_slice() else {
            return Err(unsupported(format!(
                "{} inputs (exactly one is supported)",
                job.inputs.len()
            )));
        };
        let [output] = job.outputs.as_slice() else {
            return Err(unsupported(format!(
                "{} outputs (exactly one is supported)",
                job.outputs.len()
            )));
        };
        let ext = output.path.rsplit_once('.').map_or("", |(_, e)| e);
        let output_format = FormatId::from_extension(ext)
            .ok_or_else(|| unsupported(format!("output extension {ext:?} of {}", output.path)))?;

        let (input_format, demuxer) = open_demuxer(io.open_source(&input.path)?, limits)?;
        if demuxer.streams().len() != 1 {
            return Err(unsupported(format!(
                "{} streams in {} (exactly one is supported)",
                demuxer.streams().len(),
                input.path
            )));
        }
        Self::build(
            input_format,
            output_format,
            demuxer,
            &output.audio,
            |out_stream, tags| {
                let sink = io.create_sink(&output.path)?;
                create_muxer(output_format, sink, out_stream, tags).inspect_err(|_| {
                    io.discard(&output.path);
                })
            },
        )
    }

    /// Decides copy or transcode and builds the codecs; `make_muxer` runs last, so nothing is
    /// created for the output if an earlier choice is refused.
    fn build(
        input_format: FormatId,
        output_format: FormatId,
        demuxer: Box<dyn Demuxer>,
        settings: &AudioSettings,
        make_muxer: impl FnOnce(&StreamInfo, &Tags) -> Result<Box<dyn Muxer>, Error>,
    ) -> Result<Self, Error> {
        let [stream] = demuxer.streams() else {
            return Err(unsupported("exactly one stream is supported"));
        };
        let requested = settings.codec.as_deref();
        let (out_stream, engine) = if requested == Some("copy") {
            if settings.sample_format.is_some() {
                return Err(unsupported("--codec copy cannot change the sample format"));
            }
            if settings.compression_level.is_some() {
                return Err(unsupported("--codec copy cannot take a compression level"));
            }
            if !can_hold(output_format, stream.codec) {
                return Err(unsupported(format!(
                    "cannot copy {} into {}",
                    stream.codec.name(),
                    output_format.name()
                )));
            }
            let out = StreamInfo {
                index: 0,
                ..stream.clone()
            };
            (out, Engine::Copy)
        } else {
            let in_spec = &stream.audio;
            let out_format = settings.sample_format.unwrap_or(in_spec.format());
            let codec = output_codec(output_format, out_format, requested)?;
            let options = encode_options(codec, settings.compression_level)?;
            let out_spec =
                AudioSpec::new(in_spec.sample_rate(), out_format, in_spec.layout().clone())?;
            let out = StreamInfo {
                frames: stream.frames,
                // Only meaningful while the sample format is unchanged.
                bits_per_raw_sample: stream
                    .bits_per_raw_sample
                    .filter(|_| out_format == in_spec.format()),
                ..StreamInfo::new(0, codec, out_spec.clone(), stream.time_base)
            };
            let decoder = decoder_for(stream)?;
            let encoder = encoder_for(codec, &out_spec, &options)?;
            let transcoder = Transcoder {
                decoder,
                encoder,
                decoded: AudioFrame::new(in_spec),
                converted: AudioFrame::new(&out_spec),
            };
            (out, Engine::Transcode(Box::new(transcoder)))
        };
        let plan = Plan {
            input_format,
            output_format,
            input: stream.clone(),
            output: out_stream,
            copy: matches!(engine, Engine::Copy),
        };
        let muxer = make_muxer(&plan.output, demuxer.tags())?;
        Ok(Self {
            plan,
            demuxer,
            engine,
            muxer,
            packet: Packet::default(),
            frames_done: 0,
            finished: false,
        })
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Moves one packet through the pipeline. At the end of input it drains
    /// the codecs, hands the muxer the final codec header and finishes it.
    pub fn step(&mut self) -> Result<Step, Error> {
        if self.finished {
            return Ok(Step::Finished);
        }
        if self.demuxer.read_packet(&mut self.packet)? {
            match &mut self.engine {
                Engine::Copy => {
                    self.muxer.write_packet(&self.packet)?;
                    self.frames_done += self.packet.duration;
                }
                Engine::Transcode(t) => {
                    t.decoder.send_packet(&self.packet)?;
                    self.frames_done += t.pump_frames(
                        &mut self.packet,
                        &mut *self.muxer,
                        self.plan.output.audio.format(),
                    )?;
                }
            }
            return Ok(Step::Progress {
                frames_done: self.frames_done,
                frames_total: self.plan.input.frames,
            });
        }
        if let Engine::Transcode(t) = &mut self.engine {
            t.decoder.drain();
            self.frames_done += t.pump_frames(
                &mut self.packet,
                &mut *self.muxer,
                self.plan.output.audio.format(),
            )?;
            t.encoder.drain();
            t.pump_packets(&mut self.packet, &mut *self.muxer)?;
            let extradata = t.encoder.extradata();
            if !extradata.is_empty() {
                self.muxer.set_extradata(extradata)?;
            }
        }
        self.muxer.finish()?;
        self.finished = true;
        Ok(Step::Finished)
    }
}

impl Transcoder {
    /// Decoder to encoder. The protocol wants every frame received before
    /// the next send, so this loops until the decoder is dry. Returns the frames decoded.
    fn pump_frames(
        &mut self,
        packet: &mut Packet,
        muxer: &mut dyn Muxer,
        out_format: SampleFormat,
    ) -> Result<u64, Error> {
        let mut frames = 0;
        while self.decoder.receive_frame(&mut self.decoded)? {
            frames += self.decoded.frames() as u64;
            let frame = if self.decoded.samples.format() == out_format {
                &self.decoded
            } else {
                convert_format(&self.decoded, out_format, &mut self.converted)?;
                &self.converted
            };
            self.encoder.send_frame(frame)?;
            self.pump_packets(packet, muxer)?;
        }
        Ok(frames)
    }

    fn pump_packets(&mut self, packet: &mut Packet, muxer: &mut dyn Muxer) -> Result<(), Error> {
        while self.encoder.receive_packet(packet)? {
            muxer.write_packet(packet)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
