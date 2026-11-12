//! Gapless playback: the one place the engine drops what a decoder yields outside a stream's
//! timeline (an encoder's priming before pts 0, its padding at or past `StreamInfo::frames`).

use crossfade_core::{AudioFrame, StreamInfo};

/// The part of a stream meant to be heard: frames from pts 0 up to, not including, the
/// stream's `frames` when it knows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeline {
    end: Option<u64>,
}

impl Timeline {
    pub fn new(stream: &StreamInfo) -> Self {
        Self { end: stream.frames }
    }

    /// Cuts `frame` (pts counted in frames) in place to the part on the timeline, moving its pts
    /// to where that part starts. `false` when nothing of it is left: drop it. A frame wholly on
    /// the timeline is not touched.
    pub fn trim(&self, frame: &mut AudioFrame) -> bool {
        let len = frame.frames() as i128;
        let start = i128::from(frame.pts);
        // In frames from the frame's first: where the timeline starts and where it ends.
        let skip = (-start).clamp(0, len);
        let keep = self
            .end
            .map_or(len, |end| (i128::from(end) - start).clamp(0, len));
        if skip == 0 && keep == len {
            return true;
        }
        if keep <= skip {
            frame.samples.clear();
            return false;
        }
        let channels = i128::from(frame.layout.channels());
        cut(
            &mut frame.samples,
            (skip * channels) as usize,
            (keep * channels) as usize,
        );
        // skip > 0 only for a negative pts, so this stays in range.
        frame.pts += skip as i64;
        true
    }
}

/// Keeps samples `from..to`.
fn cut(samples: &mut crossfade_core::Samples, from: usize, to: usize) {
    use crossfade_core::Samples::*;
    fn keep<T>(v: &mut Vec<T>, from: usize, to: usize) {
        v.truncate(to);
        v.drain(..from);
    }
    match samples {
        U8(v) => keep(v, from, to),
        S16(v) => keep(v, from, to),
        S24(v) | S32(v) => keep(v, from, to),
        F32(v) => keep(v, from, to),
        F64(v) => keep(v, from, to),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{
        AudioSpec, ChannelLayout, CodecId, SampleFormat, Samples, StreamInfo, TimeBase,
    };

    fn timeline(frames: Option<u64>) -> Timeline {
        let audio = AudioSpec::new(8000, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
        Timeline::new(&StreamInfo {
            frames,
            ..StreamInfo::new(0, CodecId::Mp3, audio, TimeBase::new(1, 8000).unwrap())
        })
    }

    /// A stereo frame of `len` frames at `pts` whose samples say where they sit: the left
    /// channel holds the frame's timeline position, the right its negation.
    fn frame(format: SampleFormat, pts: i64, len: i64) -> AudioFrame {
        let spec = AudioSpec::new(8000, format, ChannelLayout::stereo()).unwrap();
        let mut f = AudioFrame::new(&spec);
        f.pts = pts;
        let at = (pts..pts + len).flat_map(|p| [p, p.wrapping_neg()]);
        f.samples = match format {
            SampleFormat::U8 => Samples::U8(at.map(|p| p.wrapping_add(128) as u8).collect()),
            SampleFormat::S16 => Samples::S16(at.map(|p| p as i16).collect()),
            SampleFormat::S24 => Samples::S24(at.map(|p| p as i32).collect()),
            SampleFormat::S32 => Samples::S32(at.map(|p| p as i32).collect()),
            SampleFormat::F32 => Samples::F32(at.map(|p| p as f32).collect()),
            SampleFormat::F64 => Samples::F64(at.map(|p| p as f64).collect()),
        };
        f
    }

    const FORMATS: [SampleFormat; 6] = [
        SampleFormat::U8,
        SampleFormat::S16,
        SampleFormat::S24,
        SampleFormat::S32,
        SampleFormat::F32,
        SampleFormat::F64,
    ];

    /// What survives of a frame at `pts` of `len` frames: the frame it should equal, or `None`.
    fn check(end: Option<u64>, pts: i64, len: i64, want: Option<(i64, i64)>) {
        for format in FORMATS {
            let mut f = frame(format, pts, len);
            let kept = timeline(end).trim(&mut f);
            let case = format!("{format:?} end {end:?} pts {pts} len {len}");
            match want {
                Some((from, to)) => {
                    assert!(kept, "{case}");
                    assert_eq!(f, frame(format, from, to - from), "{case}");
                }
                None => assert!(!kept, "{case}"),
            }
        }
    }

    #[test]
    fn a_frame_on_the_timeline_is_untouched() {
        check(None, 0, 10, Some((0, 10)));
        check(Some(100), 0, 10, Some((0, 10)));
        check(Some(100), 90, 10, Some((90, 100)));
        check(None, 1 << 40, 3, Some((1 << 40, (1 << 40) + 3)));
    }

    #[test]
    fn priming_before_zero_is_dropped() {
        check(None, -10, 10, None);
        check(None, -25, 10, None);
        check(Some(100), -10, 10, None);
    }

    #[test]
    fn priming_that_spans_zero_leaves_the_rest_of_the_frame_at_zero() {
        check(None, -3, 10, Some((0, 7)));
        check(Some(100), -9, 10, Some((0, 1)));
        check(Some(100), -1, 10, Some((0, 9)));
    }

    #[test]
    fn padding_at_or_past_the_end_is_cut() {
        check(Some(100), 95, 10, Some((95, 100)));
        check(Some(100), 99, 10, Some((99, 100)));
        check(Some(100), 100, 10, None);
        check(Some(100), 250, 10, None);
        check(Some(0), 0, 10, None);
    }

    #[test]
    fn a_frame_can_lose_both_ends() {
        check(Some(4), -3, 10, Some((0, 4)));
        check(Some(1), -9, 10, Some((0, 1)));
        check(Some(5), -10, 10, None);
    }

    #[test]
    fn without_a_length_nothing_is_cut_at_the_end() {
        check(None, 0, 1 << 12, Some((0, 1 << 12)));
        check(None, i64::MAX - 10, 10, Some((i64::MAX - 10, i64::MAX)));
    }

    #[test]
    fn extreme_pts_do_not_overflow() {
        check(None, i64::MIN, 10, None);
        check(Some(u64::MAX), i64::MAX - 10, 10, Some((i64::MAX - 10, i64::MAX)));
    }

    #[test]
    fn an_untouched_frame_keeps_its_buffer() {
        let mut f = frame(SampleFormat::F32, 0, 64);
        let Samples::F32(v) = &f.samples else {
            unreachable!()
        };
        let before = v.as_ptr();
        assert!(timeline(Some(64)).trim(&mut f));
        let Samples::F32(v) = &f.samples else {
            unreachable!()
        };
        assert_eq!(v.as_ptr(), before);
    }
}
