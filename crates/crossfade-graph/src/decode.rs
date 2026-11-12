//! A decode-only pass for analysis: loudness for `probe` and `normalize`, waveforms for the
//! app. No encoder, no muxer, nothing written.

use crate::run::CancelToken;
use crate::session::DecodeSession;
use crossfade_core::{AudioFrame, Error, JobIo, Limits, SampleFormat, StreamInfo};
use crossfade_dsp::convert::convert_format;

/// Decodes the one audio stream of `path` and hands every frame on its timeline to `on_frame`
/// (priming and padding trimmed, see `Timeline`), as `f64`, in order, with pts counting frames
/// from 0. `cancel` is polled between frames; an error from `on_frame` stops the pass and is
/// returned. Returns the stream.
pub fn decode_frames(
    path: &str,
    io: &mut dyn JobIo,
    limits: &Limits,
    cancel: &CancelToken,
    on_frame: &mut dyn FnMut(&StreamInfo, &AudioFrame) -> Result<(), Error>,
) -> Result<StreamInfo, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    decode_session(DecodeSession::open(path, io, limits)?, cancel, on_frame)
}

/// [`decode_frames`] once the input is open.
fn decode_session(
    mut session: DecodeSession,
    cancel: &CancelToken,
    on_frame: &mut dyn FnMut(&StreamInfo, &AudioFrame) -> Result<(), Error>,
) -> Result<StreamInfo, Error> {
    let stream = session.stream().clone();
    let mut decoded = AudioFrame::new(&stream.audio);
    let mut working = AudioFrame::new(&stream.audio);
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if !session.next_frame(&mut decoded)? {
            return Ok(stream);
        }
        convert_format(&decoded, SampleFormat::F64, &mut working)?;
        on_frame(&stream, &working)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{AudioSpec, ChannelLayout, CodecId, Demuxer, Packet, Samples, TimeBase};
    use std::collections::VecDeque;

    struct Scripted {
        streams: [StreamInfo; 1],
        packets: VecDeque<Packet>,
    }

    impl Demuxer for Scripted {
        fn streams(&self) -> &[StreamInfo] {
            &self.streams
        }
        fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
            Ok(self.packets.pop_front().map(|p| *out = p).is_some())
        }
    }

    /// Priming and padding never reach the analysis (loudness, waveforms): the real PCM
    /// decoder stamps each frame with its packet's pts and the pass trims to the timeline.
    #[test]
    fn frames_are_trimmed_to_the_timeline() {
        let audio = AudioSpec::new(8000, SampleFormat::S16, ChannelLayout::mono()).unwrap();
        let stream = StreamInfo {
            frames: Some(3),
            ..StreamInfo::new(0, CodecId::PcmS16Le, audio, TimeBase::new(1, 8000).unwrap())
        };
        let at = |pts: i64, samples: &[i16]| Packet {
            pts,
            duration: samples.len() as u64,
            data: samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
            ..Packet::default()
        };
        let demuxer = Box::new(Scripted {
            streams: [stream],
            packets: vec![
                at(-4, &[1, 2]),
                at(-2, &[3, 4, 5, 6]),
                at(2, &[7, 8]),
                at(4, &[9]),
            ]
            .into(),
        });
        let mut seen = Vec::new();
        decode_session(
            DecodeSession::with_demuxer(demuxer, "scripted", &Limits::default()).unwrap(),
            &CancelToken::new(),
            &mut |_, f| {
                let Samples::F64(v) = &f.samples else {
                    panic!("f64 frames");
                };
                seen.push((
                    f.pts,
                    v.iter().map(|x| (x * 32768.0) as i16).collect::<Vec<_>>(),
                ));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(seen, [(0, vec![5, 6]), (2, vec![7])]);
    }
}
