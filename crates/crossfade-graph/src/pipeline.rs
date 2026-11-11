use crate::job::{AudioSettings, Job};
use crate::two_pass::{Measuring, NullMuxer, Pass};
use crate::validate::{output_format_for, resolve, resolve_join};
use crossfade_codec::decoder_for_threads;
use crossfade_core::{
    AudioFrame, Decoder, Demuxer, Encoder, Error, Filter, JobIo, Limits, Muxer, Packet,
    SampleFormat, StreamInfo, Tags,
};
use crossfade_dsp::chain::{Chain, ChainPlan};
use crossfade_dsp::convert::convert_format;
use crossfade_format::{FormatId, create_muxer, open_demuxer};
use std::collections::VecDeque;

/// What a job will do, known once the input has been opened.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub input_format: FormatId,
    pub output_format: FormatId,
    /// The input; for a join, the first one.
    pub input: StreamInfo,
    pub output: StreamInfo,
    /// Packets go straight from demuxer to muxer; no decoder or encoder exists.
    pub copy: bool,
    /// What the job assumes (see `OutputPlan::notes`).
    pub notes: Vec<String>,
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
    /// The current input's.
    decoder: Box<dyn Decoder>,
    encoder: Box<dyn Encoder>,
    /// Between the join and the encoder when the job processes; `None` is the bit-exact path.
    chain: Option<Chain>,
    /// The current input's conversion to the joined rate and layout, to `f64`.
    conform: Option<Chain>,
    decoded: AudioFrame,
    /// What `conform` gives.
    joined: AudioFrame,
    /// The format conversion's output, or the chain's.
    converted: AudioFrame,
    /// Joined frames give contiguous pts across inputs; `None` leaves a single input's alone.
    next_pts: Option<i64>,
}

/// An input of a join still to come: opened only when its turn comes.
struct Pending {
    path: String,
    /// As planned; the file must still say so when it is opened.
    stream: StreamInfo,
    conform: Option<ChainPlan>,
    decoder_threads: usize,
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
    /// Input frames, over every input.
    frames_total: Option<u64>,
    rest: VecDeque<Pending>,
    limits: Limits,
    /// A measuring pass's meter, until taken.
    measuring: Option<Measuring>,
    finished: bool,
}

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

/// The one stream of the demuxer for `path`.
fn only_stream(demuxer: &dyn Demuxer, path: &str) -> Result<StreamInfo, Error> {
    match demuxer.streams() {
        [stream] => Ok(stream.clone()),
        streams => Err(unsupported(format!(
            "{} streams in {path} (exactly one is supported)",
            streams.len()
        ))),
    }
}

impl Pipeline {
    /// One output with one audio stream, from one input or several joined in order (each
    /// input with one audio stream). Every input is read up front for its stream, one at a
    /// time; only the first stays open. If this fails after the sink exists, the sink is
    /// discarded.
    pub fn open(job: &Job, io: &mut dyn JobIo, limits: &Limits) -> Result<Self, Error> {
        Self::open_pass(job, io, limits, Pass::Convert(None))
    }

    /// [`Self::open`] for one pass of a job. A measuring pass creates no output.
    pub(crate) fn open_pass(
        job: &Job,
        io: &mut dyn JobIo,
        limits: &Limits,
        pass: Pass,
    ) -> Result<Self, Error> {
        if job.version != Job::VERSION {
            return Err(unsupported(format!("job version {}", job.version)));
        }
        let [output] = job.outputs.as_slice() else {
            return Err(unsupported(format!(
                "{} outputs (exactly one is supported)",
                job.outputs.len()
            )));
        };
        let output_format = output_format_for(&output.path)?;
        let make_muxer = |io: &mut dyn JobIo,
                          out_stream: &StreamInfo,
                          tags: &Tags|
         -> Result<Box<dyn Muxer>, Error> {
            if pass == Pass::Measure {
                return Ok(Box::new(NullMuxer));
            }
            let sink = io.create_sink(&output.path)?;
            create_muxer(output_format, sink, out_stream, tags).inspect_err(|_| {
                io.discard(&output.path);
            })
        };
        match job.inputs.as_slice() {
            [] => Err(unsupported("0 inputs (exactly one is supported)")),
            [input] => {
                let (input_format, demuxer) = open_demuxer(io.open_source(&input.path)?, limits)?;
                only_stream(&*demuxer, &input.path)?;
                let mut p = Self::build(
                    input_format,
                    output_format,
                    demuxer,
                    &output.audio,
                    limits.max_threads,
                    pass,
                    |out_stream, tags| make_muxer(io, out_stream, tags),
                )?;
                p.limits = *limits;
                Ok(p)
            }
            inputs => {
                let mut first = None;
                let mut streams = Vec::with_capacity(inputs.len());
                for input in inputs {
                    let (format, demuxer) = open_demuxer(io.open_source(&input.path)?, limits)?;
                    streams.push(only_stream(&*demuxer, &input.path)?);
                    // The others close here and open again in turn.
                    first.get_or_insert((format, demuxer));
                }
                let Some((input_format, demuxer)) = first else {
                    return Err(unsupported("0 inputs"));
                };
                let (join, codecs) = resolve_join(
                    &streams,
                    output_format,
                    &output.audio,
                    limits.max_threads,
                    pass,
                )?;
                let decoder = decoder_for_threads(&streams[0], codecs.decoder_threads[0])?;
                let conform =
                    conform_chain(&streams[0], join.conform[0].as_ref(), limits.max_threads)?;
                let frames_total = streams
                    .iter()
                    .try_fold(0u64, |sum, s| sum.checked_add(s.frames?));
                let rest = inputs
                    .iter()
                    .zip(&streams)
                    .zip(join.conform.iter().zip(&codecs.decoder_threads))
                    .skip(1)
                    .map(|((input, stream), (conform, &decoder_threads))| Pending {
                        path: input.path.clone(),
                        stream: stream.clone(),
                        conform: conform.clone(),
                        decoder_threads,
                    })
                    .collect();
                let plan = Plan {
                    input_format,
                    output_format,
                    input: streams[0].clone(),
                    output: join.plan.output,
                    copy: false,
                    notes: join.plan.notes,
                };
                let engine = Engine::Transcode(Box::new(Transcoder {
                    decoder,
                    encoder: codecs.encoder,
                    chain: codecs.main,
                    conform,
                    decoded: AudioFrame::new(&streams[0].audio),
                    joined: AudioFrame::new(&join.joined.audio),
                    converted: AudioFrame::new(&plan.output.audio),
                    next_pts: Some(0),
                }));
                let muxer = make_muxer(io, &plan.output, demuxer.tags())?;
                Ok(Self {
                    plan,
                    demuxer,
                    engine,
                    muxer,
                    packet: Packet::default(),
                    frames_done: 0,
                    frames_total,
                    rest,
                    limits: *limits,
                    measuring: codecs.measuring,
                    finished: false,
                })
            }
        }
    }

    /// Decides copy or transcode through `resolve`; `make_muxer` runs last, so nothing is
    /// created for the output if an earlier choice is refused.
    fn build(
        input_format: FormatId,
        output_format: FormatId,
        demuxer: Box<dyn Demuxer>,
        settings: &AudioSettings,
        threads: usize,
        pass: Pass,
        make_muxer: impl FnOnce(&StreamInfo, &Tags) -> Result<Box<dyn Muxer>, Error>,
    ) -> Result<Self, Error> {
        let [stream] = demuxer.streams() else {
            return Err(unsupported("exactly one stream is supported"));
        };
        let (output, codecs) = resolve(stream, output_format, settings, threads, pass)?;
        let mut measuring = None;
        let engine = match codecs {
            None => Engine::Copy,
            Some(c) => {
                measuring = c.measuring;
                Engine::Transcode(Box::new(Transcoder {
                    decoder: c.decoder,
                    encoder: c.encoder,
                    chain: c.chain,
                    conform: None,
                    decoded: AudioFrame::new(&stream.audio),
                    joined: AudioFrame::new(&stream.audio),
                    converted: AudioFrame::new(&output.output.audio),
                    next_pts: None,
                }))
            }
        };
        let plan = Plan {
            input_format,
            output_format,
            input: stream.clone(),
            output: output.output,
            copy: output.copy,
            notes: output.notes,
        };
        let muxer = make_muxer(&plan.output, demuxer.tags())?;
        Ok(Self {
            frames_total: plan.input.frames,
            plan,
            demuxer,
            engine,
            muxer,
            packet: Packet::default(),
            frames_done: 0,
            rest: VecDeque::new(),
            limits: Limits::default(),
            measuring,
            finished: false,
        })
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// Input frames over every input, when known.
    pub(crate) fn frames_total(&self) -> Option<u64> {
        self.frames_total
    }

    /// A measuring pass's meter, once it has run to the end.
    pub(crate) fn take_measuring(&mut self) -> Option<Measuring> {
        self.measuring.take()
    }

    /// Samples the output format could not hold and saturated, so far; only a processing
    /// chain can clip.
    pub fn clipped(&self) -> u64 {
        match &self.engine {
            Engine::Transcode(t) => t.chain.as_ref().map_or(0, Chain::clipped),
            Engine::Copy => 0,
        }
    }

    /// Moves one packet through the pipeline. At the end of input it drains
    /// the codecs, hands the muxer the final codec header and finishes it. A join needs `io`
    /// to open its next input: use [`Self::step_with`]; this fails there.
    pub fn step(&mut self) -> Result<Step, Error> {
        self.advance(None)
    }

    /// [`Self::step`], opening a join's next input through `io` when its turn comes.
    pub fn step_with(&mut self, io: &mut dyn JobIo) -> Result<Step, Error> {
        self.advance(Some(io))
    }

    fn progress(&self) -> Step {
        Step::Progress {
            frames_done: self.frames_done,
            frames_total: self.frames_total,
        }
    }

    fn advance(&mut self, io: Option<&mut dyn JobIo>) -> Result<Step, Error> {
        if self.finished {
            return Ok(Step::Finished);
        }
        let read = self.demuxer.read_packet(&mut self.packet);
        if let (Err(e), Engine::Transcode(t)) = (&read, &mut self.engine) {
            // A threaded decoder may still hold a frame read before this packet that fails;
            // its error came first.
            t.decoder.flush()?;
            while t.decoder.receive_frame(&mut t.decoded)? {}
            return Err(e.clone());
        }
        let out_format = self.plan.output.audio.format();
        if read? {
            match &mut self.engine {
                Engine::Copy => {
                    self.muxer.write_packet(&self.packet)?;
                    self.frames_done += self.packet.duration;
                }
                Engine::Transcode(t) => {
                    t.decoder.send_packet(&self.packet)?;
                    self.frames_done +=
                        t.pump_frames(&mut self.packet, &mut *self.muxer, out_format)?;
                    if t.chain.as_ref().is_some_and(Filter::ignores_further_input) {
                        // Nothing still to read can change the output (a trim is past its
                        // end): skip it, later inputs too, count it as done, and finish now.
                        self.frames_done = self.frames_total.unwrap_or(self.frames_done);
                        self.finish()?;
                    }
                }
            }
            return Ok(self.progress());
        }
        if let Engine::Transcode(t) = &mut self.engine {
            t.decoder.drain();
            self.frames_done += t.pump_frames(&mut self.packet, &mut *self.muxer, out_format)?;
            // The conversion's tail belongs to this input: the next one starts after it.
            if let Some(conform) = &mut t.conform {
                conform.drain();
            }
            t.pump_conform(&mut self.packet, &mut *self.muxer, out_format)?;
        }
        if let Some(next) = self.rest.pop_front() {
            let io = io
                .ok_or_else(|| unsupported("a pipeline joining inputs steps with step_with(io)"))?;
            self.open_next(io, next)?;
            return Ok(self.progress());
        }
        self.finish()?;
        Ok(Step::Finished)
    }

    /// Makes `next` the current input: its demuxer, decoder and conversion.
    fn open_next(&mut self, io: &mut dyn JobIo, next: Pending) -> Result<(), Error> {
        let (_, demuxer) = open_demuxer(io.open_source(&next.path)?, &self.limits)?;
        let stream = only_stream(&*demuxer, &next.path)?;
        if stream != next.stream {
            return Err(Error::Malformed {
                what: format!("{} changed while the join was running", next.path),
            });
        }
        let Engine::Transcode(t) = &mut self.engine else {
            return Err(unsupported("joining inputs decodes them"));
        };
        t.decoder = decoder_for_threads(&stream, next.decoder_threads)?;
        t.conform = conform_chain(&stream, next.conform.as_ref(), self.limits.max_threads)?;
        t.decoded = AudioFrame::new(&stream.audio);
        self.demuxer = demuxer;
        Ok(())
    }

    /// Drains the chain and the encoder, hands the muxer the final codec header, finishes it.
    fn finish(&mut self) -> Result<(), Error> {
        if let Engine::Transcode(t) = &mut self.engine {
            if let Some(chain) = &mut t.chain {
                chain.drain();
            }
            t.pump_chain(&mut self.packet, &mut *self.muxer)?;
            t.encoder.drain();
            t.pump_packets(&mut self.packet, &mut *self.muxer)?;
            let extradata = t.encoder.extradata();
            if !extradata.is_empty() {
                self.muxer.set_extradata(extradata)?;
            }
        }
        self.muxer.finish()?;
        self.finished = true;
        Ok(())
    }
}

fn conform_chain(
    stream: &StreamInfo,
    plan: Option<&ChainPlan>,
    threads: usize,
) -> Result<Option<Chain>, Error> {
    plan.map(|p| Chain::with_threads(&stream.audio, p, SampleFormat::F64, threads))
        .transpose()
}

/// One joined frame on to the encoder: through the main chain, or converted to the output
/// format, or as it is.
fn feed(
    frame: &AudioFrame,
    chain: &mut Option<Chain>,
    encoder: &mut dyn Encoder,
    converted: &mut AudioFrame,
    packet: &mut Packet,
    muxer: &mut dyn Muxer,
    out_format: SampleFormat,
) -> Result<(), Error> {
    if let Some(chain) = chain {
        chain.send_frame(frame)?;
        while chain.receive_frame(converted)? {
            encoder.send_frame(converted)?;
            write_packets(encoder, packet, muxer)?;
        }
        return Ok(());
    }
    let frame = if frame.samples.format() == out_format {
        frame
    } else {
        convert_format(frame, out_format, converted)?;
        converted
    };
    encoder.send_frame(frame)?;
    write_packets(encoder, packet, muxer)
}

fn write_packets(
    encoder: &mut dyn Encoder,
    packet: &mut Packet,
    muxer: &mut dyn Muxer,
) -> Result<(), Error> {
    while encoder.receive_packet(packet)? {
        muxer.write_packet(packet)?;
    }
    Ok(())
}

/// Gives a joined frame the next pts of the joined stream.
fn restamp(frame: &mut AudioFrame, next_pts: &mut Option<i64>) {
    if let Some(pts) = next_pts {
        frame.pts = *pts;
        *pts += frame.frames() as i64;
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
            if let Some(conform) = &mut self.conform {
                conform.send_frame(&self.decoded)?;
                self.pump_conform(packet, muxer, out_format)?;
                continue;
            }
            restamp(&mut self.decoded, &mut self.next_pts);
            feed(
                &self.decoded,
                &mut self.chain,
                &mut *self.encoder,
                &mut self.converted,
                packet,
                muxer,
                out_format,
            )?;
        }
        Ok(frames)
    }

    /// The current input's conversion to the encoder, until it has nothing more for now.
    fn pump_conform(
        &mut self,
        packet: &mut Packet,
        muxer: &mut dyn Muxer,
        out_format: SampleFormat,
    ) -> Result<(), Error> {
        let Some(conform) = &mut self.conform else {
            return Ok(());
        };
        while conform.receive_frame(&mut self.joined)? {
            restamp(&mut self.joined, &mut self.next_pts);
            feed(
                &self.joined,
                &mut self.chain,
                &mut *self.encoder,
                &mut self.converted,
                packet,
                muxer,
                out_format,
            )?;
        }
        Ok(())
    }

    /// Chain to encoder, until the chain has nothing more for now.
    fn pump_chain(&mut self, packet: &mut Packet, muxer: &mut dyn Muxer) -> Result<(), Error> {
        let Some(chain) = &mut self.chain else {
            return Ok(());
        };
        while chain.receive_frame(&mut self.converted)? {
            self.encoder.send_frame(&self.converted)?;
            write_packets(&mut *self.encoder, packet, muxer)?;
        }
        Ok(())
    }

    fn pump_packets(&mut self, packet: &mut Packet, muxer: &mut dyn Muxer) -> Result<(), Error> {
        write_packets(&mut *self.encoder, packet, muxer)
    }
}

#[cfg(test)]
mod tests;
