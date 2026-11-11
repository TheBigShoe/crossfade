//! A trim past its end stops the pipeline reading the source, wherever the trim sits in the
//! chain, and what comes after it (a fade-out, a resampler) still flushes: the output equals
//! doing the trim alone first and the rest afterwards.
mod common;

use common::{data_chunk, spec, wav_bytes};
use crossfade_core::{ChannelLayout, Error, JobIo, Limits, MediaSink, MediaSource, SampleFormat};
use crossfade_graph::filter::FilterSpec;
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, run};
use crossfade_testkit::memory::MemoryIo;
use crossfade_testkit::pcm;
use std::cell::Cell;
use std::io::{self, SeekFrom};
use std::rc::Rc;

/// Counts the bytes every source hands out.
struct Counting {
    inner: MemoryIo,
    read: Rc<Cell<u64>>,
}

struct CountingSource {
    inner: Box<dyn MediaSource>,
    read: Rc<Cell<u64>>,
}

impl MediaSource for CountingSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }
}

impl JobIo for Counting {
    fn open_source(&mut self, path: &str) -> Result<Box<dyn MediaSource>, Error> {
        Ok(Box::new(CountingSource {
            inner: self.inner.open_source(path)?,
            read: self.read.clone(),
        }))
    }
    fn create_sink(&mut self, path: &str) -> Result<Box<dyn MediaSink>, Error> {
        self.inner.create_sink(path)
    }
    fn commit(&mut self, path: &str) -> Result<(), Error> {
        self.inner.commit(path)
    }
    fn discard(&mut self, path: &str) {
        self.inner.discard(path);
    }
}

const FS: u32 = 44_100;

fn io() -> Counting {
    let x: Vec<f64> = (0..20 * FS as usize * 2)
        .map(|i| 0.5 * libm::sin(i as f64 * 0.0021))
        .collect();
    let mut inner = MemoryIo::new();
    inner.insert(
        "long.wav",
        wav_bytes(
            &spec(FS, SampleFormat::F64, ChannelLayout::stereo()),
            &pcm::encode_le(&x, SampleFormat::F64),
        ),
    );
    Counting {
        inner,
        read: Rc::new(Cell::new(0)),
    }
}

fn convert(io: &mut Counting, input: &str, output: &str, filters: &[&str]) -> u64 {
    io.read.set(0);
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: input.into() }],
        outputs: vec![JobOutput {
            path: output.into(),
            audio: AudioSettings {
                sample_format: Some(SampleFormat::F64),
                filters: filters
                    .iter()
                    .map(|f| f.parse::<FilterSpec>().unwrap())
                    .collect(),
                ..AudioSettings::default()
            },
        }],
    };
    run(
        &job,
        io,
        &Limits::default(),
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    io.read.get()
}

#[test]
fn a_trimmed_conversion_reads_only_what_the_trim_keeps() {
    let mut io = io();
    let file = io.inner.get("long.wav").unwrap().len() as u64;
    let whole = convert(&mut io, "long.wav", "copy.wav", &["gain 0dB"]);
    assert!(
        whole >= file - 1000,
        "a plain conversion reads it all: {whole} of {file}"
    );
    for filters in [
        &["trim end=1s"][..],
        &["trim start=0.5s end=1s", "fade out=0.25s"],
        &["trim end=1s", "resample 48000"],
        &["gain -1dB", "trim end=1s"],
        &["resample 48000", "trim end=1s", "fade out=100ms"],
    ] {
        let read = convert(&mut io, "long.wav", "out.wav", filters);
        assert!(read < file / 10, "{filters:?}: read {read} of {file} bytes");
    }
}

#[test]
fn what_follows_the_trim_still_flushes_as_if_the_trim_ran_alone_first() {
    let mut io = io();
    for (together, first, then) in [
        (
            &["trim start=0.5s end=1s", "fade out=0.25s"][..],
            "trim start=0.5s end=1s",
            &["fade out=0.25s"][..],
        ),
        (
            &["trim end=1s", "resample 48000"],
            "trim end=1s",
            &["resample 48000"],
        ),
        (
            &["trim end=1s", "resample 48000", "fade out=100ms"],
            "trim end=1s",
            &["resample 48000", "fade out=100ms"],
        ),
    ] {
        convert(&mut io, "long.wav", "together.wav", together);
        convert(&mut io, "long.wav", "first.wav", &[first]);
        convert(&mut io, "first.wav", "then.wav", then);
        let get = |p: &str| io.inner.get(p).unwrap();
        assert_eq!(
            data_chunk(&get("together.wav")),
            data_chunk(&get("then.wav")),
            "{together:?}"
        );
    }
}
