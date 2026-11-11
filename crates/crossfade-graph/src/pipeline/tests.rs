//! Pipeline wiring with test doubles: the engine's real codecs and containers are not needed to
//! see that tags, extradata and stream copy reach the right place.

use super::*;
use crate::job::AudioSettings;
use crossfade_core::{AudioSpec, ChannelLayout, CodecId, SampleFormat, Samples, TimeBase};
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
}

impl Muxer for RecordingMuxer {
    fn write_packet(&mut self, p: &Packet) -> Result<(), Error> {
        self.log.borrow_mut().push(format!("packet:{:?}", p.data));
        Ok(())
    }
    fn set_extradata(&mut self, extradata: &[u8]) -> Result<(), Error> {
        self.log
            .borrow_mut()
            .push(format!("extradata:{extradata:?}"));
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
        out.samples = Samples::S16(
            d.chunks(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect(),
        );
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
    Pipeline::build(FormatId::Wav, format, d, s, 0, move |_, tags| {
        *seen.borrow_mut() = Some(tags.clone());
        Ok(Box::new(RecordingMuxer { log }))
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
        unsupported(attempt(
            CodecId::PcmS16Le,
            FormatId::Aiff,
            settings(Some("copy"))
        )),
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

/// Reads `ok` packets, then fails.
struct FailingDemuxer {
    streams: Vec<StreamInfo>,
    tags: Tags,
    ok: usize,
}

impl Demuxer for FailingDemuxer {
    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }
    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        if self.ok == 0 {
            return Err(Error::Malformed {
                what: "reader".into(),
            });
        }
        self.ok -= 1;
        *out = packet(&[1, 0], 1);
        Ok(true)
    }
    fn tags(&self) -> &Tags {
        &self.tags
    }
}

/// A decoder still working on what it was sent (as a threaded one is), which finds a bad frame
/// only when asked to hand over everything.
struct LaggingDecoder {
    flushed: bool,
}

impl Decoder for LaggingDecoder {
    fn send_packet(&mut self, _: &Packet) -> Result<(), Error> {
        Ok(())
    }
    fn receive_frame(&mut self, _: &mut AudioFrame) -> Result<bool, Error> {
        Ok(false)
    }
    fn drain(&mut self) {}
    fn flush(&mut self) -> Result<(), Error> {
        self.flushed = true;
        Err(Error::Malformed {
            what: "an earlier frame".into(),
        })
    }
}

/// A reader error after packets the decoder has not finished with: the decoder's error about
/// an earlier frame wins, as it would have one frame at a time.
#[test]
fn a_reader_error_reports_a_decoding_error_from_before_it_first() {
    let (log, seen) = (Log::default(), Rc::default());
    let d: Box<dyn Demuxer> = Box::new(FailingDemuxer {
        streams: vec![stream(CodecId::PcmS16Le)],
        tags: Tags::new(),
        ok: 2,
    });
    let mut p = build(FormatId::Wav, d, &settings(None), &log, &seen).unwrap();
    let Engine::Transcode(t) = &mut p.engine else {
        panic!("not copying");
    };
    t.decoder = Box::new(LaggingDecoder { flushed: false });
    assert!(p.step().is_ok());
    assert!(p.step().is_ok());
    assert_eq!(
        p.step(),
        Err(Error::Malformed {
            what: "an earlier frame".into()
        })
    );
    // A decoder with nothing held back leaves the reader's error as it was.
    let d: Box<dyn Demuxer> = Box::new(FailingDemuxer {
        streams: vec![stream(CodecId::PcmS16Le)],
        tags: Tags::new(),
        ok: 1,
    });
    let mut p = build(FormatId::Wav, d, &settings(None), &log, &seen).unwrap();
    assert!(p.step().is_ok());
    assert_eq!(
        p.step(),
        Err(Error::Malformed {
            what: "reader".into()
        })
    );
}

/// Counts the packets read, to see a pipeline stop reading.
struct CountingDemuxer {
    inner: Box<dyn Demuxer>,
    reads: Rc<std::cell::Cell<usize>>,
}

impl Demuxer for CountingDemuxer {
    fn streams(&self) -> &[StreamInfo] {
        self.inner.streams()
    }
    fn read_packet(&mut self, out: &mut Packet) -> Result<bool, Error> {
        self.reads.set(self.reads.get() + 1);
        self.inner.read_packet(out)
    }
}

/// Passes frames through; ignores further input once `limit` frames have gone by.
struct Upto {
    spec: AudioSpec,
    seen: u64,
    limit: u64,
    held: Option<AudioFrame>,
}

impl Filter for Upto {
    fn output_spec(&self) -> &AudioSpec {
        &self.spec
    }
    fn send_frame(&mut self, frame: &AudioFrame) -> Result<(), Error> {
        self.seen += frame.frames() as u64;
        self.held = Some(frame.clone());
        Ok(())
    }
    fn receive_frame(&mut self, out: &mut AudioFrame) -> Result<bool, Error> {
        match self.held.take() {
            Some(f) => {
                *out = f;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    fn drain(&mut self) {}
    fn ignores_further_input(&self) -> bool {
        self.seen >= self.limit
    }
}

#[test]
fn reading_stops_once_the_chain_ignores_further_input() {
    let (log, seen) = (Log::default(), Rc::default());
    let reads = Rc::new(std::cell::Cell::new(0));
    let packets = (0..10).map(|i| packet(&[i, 0, i, 0], 2)).collect();
    let d = Box::new(CountingDemuxer {
        inner: demuxer(CodecId::PcmS16Le, Tags::new(), packets),
        reads: reads.clone(),
    });
    let mut p = build(FormatId::Wav, d, &settings(None), &log, &seen).unwrap();
    let Engine::Transcode(t) = &mut p.engine else {
        panic!("not copying");
    };
    t.decoder = Box::new(ScriptedDecoder(None));
    t.encoder = Box::new(ScriptedEncoder {
        pending: None,
        extradata: Vec::new(),
    });
    let f64_spec = AudioSpec::new(8000, SampleFormat::F64, ChannelLayout::mono()).unwrap();
    let stage = Box::new(Upto {
        spec: f64_spec,
        seen: 0,
        limit: 4,
        held: None,
    });
    t.chain = Some(
        Chain::with_filters(
            &spec(),
            vec![stage],
            SampleFormat::S16,
            crossfade_dsp::dither::Dither::None,
        )
        .unwrap(),
    );
    let steps = run_to_end(&mut p);
    assert_eq!(
        reads.get(),
        2,
        "two packets give the four frames the filter wants"
    );
    // Progress ends at the whole input: what was skipped counts as done.
    assert_eq!(
        steps[steps.len() - 2..],
        [
            Step::Progress {
                frames_done: 1000,
                frames_total: Some(1000)
            },
            Step::Finished
        ]
    );
    assert_eq!(
        log.borrow().as_slice(),
        ["packet:[0, 0, 0, 0]", "packet:[1, 0, 1, 0]", "finish"]
    );
}
