//! A decode-only pass for analysis: loudness for `probe` and `normalize`, waveforms for the
//! app. No encoder, no muxer, nothing written.

use crate::run::CancelToken;
use crossfade_codec::decoder_for_threads;
use crossfade_core::{AudioFrame, Demuxer, Error, JobIo, Limits, Packet, SampleFormat, StreamInfo};
use crossfade_dsp::convert::convert_format;
use crossfade_format::open_demuxer;

/// Decodes the one audio stream of `path` and hands every frame to `on_frame`, as `f64`, in
/// order, with pts counting frames from 0. `cancel` is polled between packets; an error from
/// `on_frame` stops the pass and is returned. Returns the stream.
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
    let (_, demuxer) = open_demuxer(io.open_source(path)?, limits)?;
    decode_demuxed(demuxer, path, limits, cancel, on_frame)
}

/// [`decode_frames`] once `path` is open.
pub(crate) fn decode_demuxed(
    mut demuxer: Box<dyn Demuxer>,
    path: &str,
    limits: &Limits,
    cancel: &CancelToken,
    on_frame: &mut dyn FnMut(&StreamInfo, &AudioFrame) -> Result<(), Error>,
) -> Result<StreamInfo, Error> {
    let [stream] = demuxer.streams() else {
        return Err(Error::Unsupported {
            what: format!(
                "{} streams in {path} (exactly one is supported)",
                demuxer.streams().len()
            ),
        });
    };
    let stream = stream.clone();
    let mut decoder = decoder_for_threads(&stream, limits.max_threads)?;
    let mut packet = Packet::default();
    let mut decoded = AudioFrame::new(&stream.audio);
    let mut working = AudioFrame::new(&stream.audio);
    let mut deliver = |decoder: &mut dyn crossfade_core::Decoder| -> Result<(), Error> {
        while decoder.receive_frame(&mut decoded)? {
            convert_format(&decoded, SampleFormat::F64, &mut working)?;
            on_frame(&stream, &working)?;
        }
        Ok(())
    };
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        match demuxer.read_packet(&mut packet) {
            Ok(true) => {
                decoder.send_packet(&packet)?;
                deliver(&mut *decoder)?;
            }
            Ok(false) => break,
            Err(e) => {
                // A frame read before this packet may hold an earlier error (see `Pipeline`).
                decoder.flush()?;
                deliver(&mut *decoder)?;
                return Err(e);
            }
        }
    }
    decoder.drain();
    deliver(&mut *decoder)?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossfade_core::{AudioSpec, ChannelLayout, CodecId, Samples, TimeBase};
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
        decode_demuxed(
            demuxer,
            "scripted",
            &Limits::default(),
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
