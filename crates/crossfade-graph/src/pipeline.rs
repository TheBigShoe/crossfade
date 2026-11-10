use crate::job::Job;
use crossfade_codec::{decoder_for, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, CodecId, Decoder, Demuxer, Encoder, Error, JobIo, Limits, Muxer, Packet,
    StreamInfo,
};
use crossfade_format::{FormatId, create_muxer, open_demuxer};

/// What a job will do, known once the input has been opened.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub input_format: FormatId,
    pub output_format: FormatId,
    pub input: StreamInfo,
    pub output: StreamInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Progress {
        frames_done: u64,
        frames_total: Option<u64>,
    },
    Finished,
}

pub struct Pipeline {
    plan: Plan,
    demuxer: Box<dyn Demuxer>,
    decoder: Box<dyn Decoder>,
    encoder: Box<dyn Encoder>,
    muxer: Box<dyn Muxer>,
    packet: Packet,
    decoded: AudioFrame,
    converted: AudioFrame,
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
        let [stream] = demuxer.streams() else {
            return Err(unsupported(format!(
                "{} streams in {} (exactly one is supported)",
                demuxer.streams().len(),
                input.path
            )));
        };
        let in_spec = &stream.audio;
        let out_format = output.audio.sample_format.unwrap_or(in_spec.format());
        let out_spec = AudioSpec::new(in_spec.sample_rate(), out_format, in_spec.layout().clone())?;
        let out_stream = StreamInfo {
            index: 0,
            codec: CodecId::pcm_for(out_format),
            audio: out_spec.clone(),
            time_base: stream.time_base,
            frames: stream.frames,
        };
        let decoder = decoder_for(stream)?;
        let encoder = encoder_for(out_stream.codec, &out_spec)?;
        let plan = Plan {
            input_format,
            output_format,
            input: stream.clone(),
            output: out_stream,
        };
        let (decoded, converted) = (AudioFrame::new(in_spec), AudioFrame::new(&out_spec));

        let sink = io.create_sink(&output.path)?;
        let muxer = match create_muxer(output_format, sink, &plan.output) {
            Ok(m) => m,
            Err(e) => {
                io.discard(&output.path);
                return Err(e);
            }
        };
        Ok(Self {
            plan,
            demuxer,
            decoder,
            encoder,
            muxer,
            packet: Packet::default(),
            decoded,
            converted,
            frames_done: 0,
            finished: false,
        })
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }
}
