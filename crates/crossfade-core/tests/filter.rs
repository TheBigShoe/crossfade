//! The filter contract: frames in, frames out, the output spec fixed at construction, and pts
//! that count output frames without gaps. A toy decimator (keeps every second frame, halves
//! the rate) exercises the parts a gain cannot: a new rate, a held-back frame and new pts.

use crossfade_core::{AudioFrame, AudioSpec, ChannelLayout, Error, Filter, SampleFormat, Samples};

struct Decimate {
    out: AudioSpec,
    /// Input frames seen so far; odd ones are dropped.
    seen: u64,
    next_pts: Option<i64>,
    ready: Option<Vec<f64>>,
}

impl Decimate {
    fn new(input: &AudioSpec) -> Self {
        let out = AudioSpec::new(
            input.sample_rate() / 2,
            input.format(),
            input.layout().clone(),
        )
        .unwrap();
        Self {
            out,
            seen: 0,
            next_pts: None,
            ready: None,
        }
    }
}

impl Filter for Decimate {
    fn output_spec(&self) -> &AudioSpec {
        &self.out
    }

    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        let Samples::F64(s) = &frame.samples else {
            return Err(Error::Unsupported {
                what: "f64 only".into(),
            });
        };
        let ch = usize::from(frame.layout.channels());
        self.next_pts.get_or_insert(frame.pts / 2);
        let mut kept = Vec::new();
        for f in s.chunks(ch) {
            if self.seen.is_multiple_of(2) {
                kept.extend_from_slice(f);
            }
            self.seen += 1;
        }
        if !kept.is_empty() {
            self.ready = Some(kept);
        }
        Ok(())
    }

    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        let Some(kept) = self.ready.take() else {
            return Ok(false);
        };
        let pts = self.next_pts.unwrap_or(0);
        out.sample_rate = self.out.sample_rate();
        out.layout.clone_from(self.out.layout());
        out.pts = pts;
        out.samples = Samples::F64(kept);
        self.next_pts = Some(pts + out.frames() as i64);
        Ok(true)
    }

    /// Holds nothing back, so there is nothing to flush.
    fn drain(&mut self) {}
}

fn frame(spec: &AudioSpec, pts: i64, samples: Vec<f64>) -> AudioFrame {
    AudioFrame {
        pts,
        samples: Samples::F64(samples),
        ..AudioFrame::new(spec)
    }
}

#[test]
fn a_filter_is_driven_like_a_codec_and_its_pts_count_output_frames() {
    let input = AudioSpec::new(48000, SampleFormat::F64, ChannelLayout::mono()).unwrap();
    let mut f: Box<dyn Filter> = Box::new(Decimate::new(&input));
    assert_eq!(f.output_spec().sample_rate(), 24000);
    let mut got = Vec::new();
    let mut out = AudioFrame::new(f.output_spec());
    for (pts, chunk) in [(0, vec![0.0, 1.0, 2.0]), (3, vec![3.0, 4.0, 5.0, 6.0])] {
        f.send_frame(&frame(&input, pts, chunk)).unwrap();
        while f.receive_frame(&mut out).unwrap() {
            got.push((out.pts, out.samples.clone(), out.sample_rate));
        }
    }
    f.drain();
    assert!(!f.receive_frame(&mut out).unwrap());
    assert_eq!(
        got,
        [
            (0, Samples::F64(vec![0.0, 2.0]), 24000),
            (2, Samples::F64(vec![4.0, 6.0]), 24000),
        ]
    );
}

#[test]
fn filters_are_object_safe() {
    fn _boxed(_: Box<dyn Filter>) {}
}

/// Passes frames through; says it ignores further input once it has seen `limit` frames.
struct Upto {
    spec: AudioSpec,
    seen: u64,
    limit: u64,
}

impl Filter for Upto {
    fn output_spec(&self) -> &AudioSpec {
        &self.spec
    }
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        self.seen += frame.frames() as u64;
        Ok(())
    }
    fn receive_frame(&mut self, _: &mut AudioFrame) -> Result<bool, Error> {
        Ok(false)
    }
    fn drain(&mut self) {}
    fn ignores_further_input(&self) -> bool {
        self.seen >= self.limit
    }
}

#[test]
fn a_filter_needs_all_its_input_unless_it_says_otherwise() {
    let spec = AudioSpec::new(8000, SampleFormat::F64, ChannelLayout::mono()).unwrap();
    // The default: every filter wants its whole input.
    assert!(!Decimate::new(&spec).ignores_further_input());
    let mut f = Upto {
        spec: spec.clone(),
        seen: 0,
        limit: 3,
    };
    f.send_frame(&frame(&spec, 0, vec![0.0; 2])).unwrap();
    assert!(!f.ignores_further_input());
    f.send_frame(&frame(&spec, 2, vec![0.0; 2])).unwrap();
    assert!(f.ignores_further_input());
}
