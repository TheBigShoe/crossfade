//! The decode thread: carries out loads and seeks, and keeps the current segment's ring full.

use super::convert::Converter;
use super::feed::{Ends, Segment, SegmentState};
use super::{Load, OutputFormat, Shared, lock};
use crossfade_core::{AudioFrame, JobIo, Limits, StreamInfo};
use crossfade_graph::DecodeSession;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, TryRecvError};

/// How far the worker decodes ahead, in output frames.
fn buffer_frames(format: OutputFormat) -> usize {
    (format.sample_rate as usize / 2).max(4096)
}

pub(crate) enum Command {
    Load { path: String, generation: u64 },
    Seek { frame: u64, generation: u64 },
    Quit,
}

/// One segment being written.
struct Writer {
    converter: Converter,
    samples: rtrb::Producer<f32>,
    /// Converted samples not yet in the ring, from `at` on.
    pending: Vec<f32>,
    at: usize,
    state: Arc<SegmentState>,
    /// The session has ended and the converter drained; once `pending` is out, so is the
    /// segment.
    finished: bool,
    end: u64,
    channels: usize,
}

impl Writer {
    /// Moves what fits (whole frames) into the ring; `true` when nothing is left over.
    fn flush(&mut self) -> bool {
        let left = self.pending.len() - self.at;
        let n = left.min(self.samples.slots()) / self.channels * self.channels;
        if n > 0
            && self
                .samples
                .push_entire_slice(&self.pending[self.at..self.at + n])
                .is_ok()
        {
            self.at += n;
        }
        self.at == self.pending.len()
    }
}

struct Source {
    session: DecodeSession,
    stream: StreamInfo,
    writer: Option<Writer>,
}

enum Step {
    Busy,
    Idle,
}

pub(crate) struct Worker {
    commands: Receiver<Command>,
    shared: Arc<Shared>,
    io: Box<dyn JobIo + Send>,
    limits: Limits,
    format: OutputFormat,
    ends: Ends,
    /// A segment the feed has no room for yet.
    outbox: Option<Segment>,
    source: Option<Source>,
    frame: AudioFrame,
    notify: Box<dyn Fn() + Send + Sync>,
    handled: u64,
    last_ended: u64,
}

impl Worker {
    pub(crate) fn new(
        commands: Receiver<Command>,
        shared: Arc<Shared>,
        io: Box<dyn JobIo + Send>,
        limits: Limits,
        format: OutputFormat,
        ends: Ends,
        notify: Box<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            commands,
            shared,
            io,
            limits,
            format,
            ends,
            outbox: None,
            source: None,
            frame: AudioFrame {
                sample_rate: 1,
                layout: crossfade_core::ChannelLayout::mono(),
                pts: 0,
                samples: crossfade_core::Samples::F32(Vec::new()),
            },
            notify,
            handled: 0,
            last_ended: 0,
        }
    }

    pub(crate) fn run(mut self) {
        loop {
            let control = Arc::clone(&self.shared.control);
            control.wake.store(false, Ordering::SeqCst);
            let seen = control.requests.load(Ordering::SeqCst);
            while self.ends.retired.pop().is_ok() {}
            loop {
                match self.commands.try_recv() {
                    Ok(Command::Quit) | Err(TryRecvError::Disconnected) => return,
                    Ok(command) => {
                        self.handle(command);
                        self.handled += 1;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            self.send_outbox();
            let ended = control.ended.load(Ordering::Acquire);
            if ended != self.last_ended {
                self.last_ended = ended;
                (self.notify)();
            }
            if let Step::Idle = self.step() {
                {
                    let mut status = lock(&self.shared.status);
                    status.handled = self.handled;
                    status.seen = seen;
                }
                self.shared.idle.notify_all();
                std::thread::park();
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Load { path, generation } => {
                self.source = None;
                let opened = DecodeSession::open(&path, &mut *self.io, &self.limits);
                let load = match opened {
                    Ok(session) => {
                        let stream = session.stream().clone();
                        {
                            let mut status = lock(&self.shared.status);
                            status.stream = Some(stream.clone());
                            status.duration = stream.frames;
                        }
                        self.source = Some(Source {
                            session,
                            stream,
                            writer: None,
                        });
                        match self.start_segment(generation, 0) {
                            Ok(()) => Load::Ready,
                            Err(why) => {
                                self.source = None;
                                Load::Failed(why)
                            }
                        }
                    }
                    Err(e) => Load::Failed(e.to_string()),
                };
                lock(&self.shared.status).load = Some((generation, load));
                (self.notify)();
            }
            Command::Seek { frame, generation } => {
                let Some(source) = &mut self.source else {
                    return;
                };
                if let Err(e) = source.session.seek(frame) {
                    super::tracing_free_log(&format!("seek: {e}"));
                    return;
                }
                let at = source.session.position();
                if let Err(why) = self.start_segment(generation, at) {
                    super::tracing_free_log(&why);
                }
            }
            Command::Quit => {}
        }
    }

    /// A fresh ring from source frame `start`, sent to the feed as soon as it has room.
    fn start_segment(&mut self, generation: u64, start: u64) -> Result<(), String> {
        let Some(source) = &mut self.source else {
            return Ok(());
        };
        let converter =
            Converter::new(&source.stream.audio, self.format).map_err(|e| e.to_string())?;
        let channels = usize::from(self.format.channels);
        let (samples, reader) = rtrb::RingBuffer::new(buffer_frames(self.format) * channels);
        let state = Arc::new(SegmentState::default());
        source.writer = Some(Writer {
            converter,
            samples,
            pending: Vec::new(),
            at: 0,
            state: Arc::clone(&state),
            finished: false,
            end: start,
            channels,
        });
        // An older segment the feed never took is simply dropped here.
        self.outbox = Some(Segment::new(
            generation,
            start,
            source.stream.audio.sample_rate(),
            reader,
            state,
        ));
        self.send_outbox();
        Ok(())
    }

    fn send_outbox(&mut self) {
        if let Some(segment) = self.outbox.take()
            && let Err(rtrb::PushError::Full(segment)) = self.ends.outgoing.push(segment)
        {
            self.outbox = Some(segment);
        }
    }

    /// One decoded frame's worth of work, or `Idle` when the ring is full or the segment done.
    fn step(&mut self) -> Step {
        let Some(source) = &mut self.source else {
            return Step::Idle;
        };
        let Some(writer) = &mut source.writer else {
            return Step::Idle;
        };
        if writer.state.eof.load(Ordering::Relaxed) {
            return Step::Idle;
        }
        if !writer.flush() {
            return Step::Idle;
        }
        if writer.finished {
            writer.state.end.store(writer.end, Ordering::Release);
            writer.state.eof.store(true, Ordering::Release);
            return Step::Idle;
        }
        writer.pending.clear();
        writer.at = 0;
        let converted = match source.session.next_frame(&mut self.frame) {
            Ok(true) => writer.converter.push(&self.frame, &mut writer.pending),
            Ok(false) => {
                writer.finished = true;
                writer.end = source.session.position();
                let mut status = lock(&self.shared.status);
                status.duration.get_or_insert(writer.end);
                drop(status);
                writer.converter.drain(&mut writer.pending)
            }
            Err(e) => Err(e),
        };
        if let Err(e) = converted {
            // A preview stops where the file stops making sense; what came before still plays.
            super::tracing_free_log(&format!("decode: {e}"));
            writer.pending.clear();
            writer.finished = true;
            writer.end = source.session.position();
        }
        Step::Busy
    }
}
