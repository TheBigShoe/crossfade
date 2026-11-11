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
    draining: bool,
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
            draining: false,
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
            if self.seen % 2 == 0 {
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

    fn drain(&mut self) {
        self.draining = true;
    }
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
