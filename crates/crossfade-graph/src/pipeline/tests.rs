//! Pipeline wiring with test doubles: the engine's real codecs and containers are not needed to
//! see that tags, extradata and stream copy reach the right place.

use super::*;
use crate::job::AudioSettings;
use crossfade_core::{ChannelLayout, SampleFormat, Samples, TimeBase};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

type Log = Rc<RefCell<Vec<String>>>;

struct ScriptedDemuxer {
    streams: Vec<StreamInfo>,
    tags: Tags,
    packets: VecDeque<Packet>,
}

impl Demuxer for ScriptedDemuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        match self.packets.pop_front() {
            Some(p) => {
                *out = p;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    fn tags(&self) -> &Tags {
        &self.tags
    }
}

struct RecordingMuxer {
    log: Log,
    tags: Tags,
}

impl Muxer for RecordingMuxer {
    fn write_packet(&mut self, p: &Packet) -> Result<(), Error> {
        self.log.borrow_mut().push(format!("packet:{:?}", p.data));
        Ok(())
    }
    fn set_extradata(&mut self, extradata: &[u8]) -> Result<(), Error> {
        self.log.borrow_mut().push(format!("extradata:{extradata:?}"));
        Ok(())
    }
    fn finish(&mut self) -> Result<(), Error> {
        self.log.borrow_mut().push("finish".into());
        Ok(())
    }
}

/// One S16 frame per packet byte pair, passed straight through.
struct ScriptedDecoder(Option<Vec<u8>>);

impl Decoder for ScriptedDecoder {
    fn send_packet(&mut self, p: &Packet) -> Result<(), Error> {
        self.0 = Some(p.data.clone());
        Ok(())
    }
    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        let Some(d) = self.0.take() else {
            return Ok(false);
        };
        out.samples = Samples::S16(d.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect());
        Ok(true)
    }
    fn drain(&mut self) {}
}

struct ScriptedEncoder {
    pending: Option<Vec<u8>>,
    extradata: Vec<u8>,
}

impl Encoder for ScriptedEncoder {
    fn send_frame(&mut self, f: &AudioFrame) -> Result<(), Error> {
        let Samples::S16(v) = &f.samples else {
            panic!("S16 only");
        };
        self.pending = Some(v.iter().flat_map(|s| s.to_le_bytes()).collect());
        Ok(())
    }
    fn receive_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        match self.pending.take() {
            Some(d) => {
                out.data = d;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    fn drain(&mut self) {}
    fn extradata(&self) -> &[u8] {
        &self.extradata
    }
}

fn spec() -> AudioSpec {
    AudioSpec::new(8000, SampleFormat::S16, ChannelLayout::mono()).unwrap()
}

fn stream(codec: CodecId) -> StreamInfo {
    StreamInfo {
        frames: Some(1000),
        extradata: vec![7, 7],
        ..StreamInfo::new(0, codec, spec(), TimeBase::new(1, 8000).unwrap())
    }
}

fn packet(data: &[u8], duration: u64) -> Packet {
    Packet {
        data: data.to_vec(),
        duration,
        ..Packet::default()
    }
}

fn demuxer(codec: CodecId, tags: Tags, packets: Vec<Packet>) -> Box<dyn Demuxer> {
    Box::new(ScriptedDemuxer {
        streams: vec![stream(codec)],
        tags,
        packets: packets.into(),
    })
}

fn settings(codec: Option<&str>) -> AudioSettings {
    AudioSettings {
        codec: codec.map(String::from),
        ..AudioSettings::default()
    }
}

fn build(
    format: FormatId,
    d: Box<dyn Demuxer>,
    s: &AudioSettings,
    log: &Log,
    seen_tags: &Rc<RefCell<Option<Tags>>>,
) -> Result<Pipeline, Error> {
    let (log, seen) = (log.clone(), seen_tags.clone());
    Pipeline::build(FormatId::Wav, format, d, s, move |_, tags| {
        *seen.borrow_mut() = Some(tags.clone());
        Ok(Box::new(RecordingMuxer {
            log,
            tags: tags.clone(),
        }))
    })
}

fn run_to_end(p: &mut Pipeline) -> Vec<Step> {
    let mut steps = Vec::new();
    loop {
        let s = p.step().unwrap();
        steps.push(s);
        if s == Step::Finished {
            return steps;
        }
    }
}

fn unsupported(r: Result<impl Sized, Error>) -> String {
    match r {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("wrong error {e:?}"),
        Ok(_) => panic!("expected Unsupported"),
    }
}

#[test]
fn demuxer_tags_are_handed_to_the_muxer_factory() {
    let mut tags = Tags::new();
    tags.push("ARTIST", "Someone");
    tags.push("TITLE", "Something");
    let (log, seen) = (Log::default(), Rc::default());
    let d = demuxer(CodecId::PcmS16Le, tags.clone(), vec![]);
    build(FormatId::Wav, d, &settings(None), &log, &seen).unwrap();
    assert_eq!(seen.borrow().as_ref(), Some(&tags));
}

#[test]
fn encoder_extradata_reaches_the_muxer_once_after_the_last_packet_and_before_finish() {
    let (log, seen) = (Log::default(), Rc::default());
    let d = demuxer(
        CodecId::PcmS16Le,
        Tags::new(),
        vec![packet(&[1, 0, 2, 0], 2), packet(&[3, 0], 1)],
    );
    let mut p = build(FormatId::Wav, d, &settings(None), &log, &seen).unwrap();
    // Swap in a codec pair whose encoder has a header to report.
    let Engine::Transcode(t) = &mut p.engine else {
        panic!("not copying");
    };
    t.decoder = Box::new(ScriptedDecoder(None));
    t.encoder = Box::new(ScriptedEncoder {
        pending: None,
        extradata: vec![0xAA, 0xBB],
    });
    run_to_end(&mut p);
    assert_eq!(
        *log.borrow(),
        [
            "packet:[1, 0, 2, 0]",
            "packet:[3, 0]",
            "extradata:[170, 187]",
            "finish"
        ]
    );
}

#[test]
fn an_empty_extradata_is_not_forwarded() {
    let (log, seen) = (Log::default(), Rc::default());
    let d = demuxer(CodecId::PcmS16Le, Tags::new(), vec![packet(&[1, 0], 1)]);
    let mut p = build(FormatId::Wav, d, &settings(None), &log, &seen).unwrap();
    run_to_end(&mut p);
    assert_eq!(*log.borrow(), ["packet:[1, 0]", "finish"]);
}

#[test]
fn copy_moves_packets_without_building_a_codec() {
    // Flac has no decoder yet, so building one would fail this test.
    let (log, seen) = (Log::default(), Rc::default());
    let d = demuxer(
        CodecId::Flac,
        Tags::new(),
        vec![packet(&[1], 100), packet(&[2], 50)],
    );
    let mut p = build(FormatId::Flac, d, &settings(Some("copy")), &log, &seen).unwrap();
    assert!(p.plan().copy);
    assert_eq!(p.plan().output.codec, CodecId::Flac);
    assert_eq!(p.plan().output.extradata, [7, 7]);
    assert_eq!(p.plan().output.frames, Some(1000));
    let steps = run_to_end(&mut p);
    let done: Vec<u64> = steps
        .iter()
        .filter_map(|s| match s {
            Step::Progress { frames_done, .. } => Some(*frames_done),
            Step::Finished => None,
        })
        .collect();
    assert_eq!(done, [100, 150]);
    assert_eq!(*log.borrow(), ["packet:[1]", "packet:[2]", "finish"]);
}

#[test]
fn copy_is_refused_with_other_encoding_settings_or_an_unfit_container() {
    let attempt = |codec: CodecId, format: FormatId, s: AudioSettings| {
        let d = demuxer(codec, Tags::new(), vec![]);
        build(format, d, &s, &Log::default(), &Rc::default())
    };
    let with = |f: fn(&mut AudioSettings)| {
        let mut s = settings(Some("copy"));
        f(&mut s);
        s
    };
    assert!(
        unsupported(attempt(
            CodecId::PcmS16Le,
            FormatId::Wav,
            with(|s| s.sample_format = Some(SampleFormat::S24))
        ))
        .contains("sample")
    );
    assert!(
        unsupported(attempt(
            CodecId::Flac,
            FormatId::Flac,
            with(|s| s.compression_level = Some(5))
        ))
        .contains("compression")
    );
    assert_eq!(
        unsupported(attempt(CodecId::PcmS16Le, FormatId::Aiff, settings(Some("copy")))),
        "cannot copy pcm_s16le into aiff"
    );
    assert!(attempt(CodecId::PcmS16Le, FormatId::Wav, settings(Some("copy"))).is_ok());
}

#[test]
fn compression_level_is_checked_against_the_codec() {
    let d = || demuxer(CodecId::PcmS16Le, Tags::new(), vec![]);
    let s = AudioSettings {
        compression_level: Some(5),
        ..AudioSettings::default()
    };
    let r = build(FormatId::Wav, d(), &s, &Log::default(), &Rc::default());
    assert_eq!(unsupported(r), "--compression-level applies to flac only");
}
