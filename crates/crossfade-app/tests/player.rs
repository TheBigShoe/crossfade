//! Preview playback through a virtual sink. Nothing here sleeps or reads the wall clock: the
//! sink pulls when the test says (by count, or by a manual clock), and `Player::settle` waits
//! until the decode thread has done everything it can, so what a pull returns is decided.

mod common;

use common::{spec, wav_bytes};
use crossfade_app::player::{
    AudioSink, OutputFormat, Player, State, VirtualOutput, VirtualSink, clock_text, device_layout,
    transport_text,
};
use crossfade_core::{
    AudioFrame, ChannelLayout, Error, Filter, JobIo, Limits, MediaSink, MediaSource, SampleFormat,
    Samples,
};
use crossfade_dsp::chain::{Chain, ChainPlan};
use crossfade_dsp::convert::convert_format;
use crossfade_dsp::dither::Dither;
use crossfade_dsp::filters::FilterPlan;
use crossfade_dsp::filters::mix::MixPlan;
use crossfade_dsp::filters::resample::{Quality, ResamplePlan};
use crossfade_graph::{AudioSettings, CancelToken, DecodeSession, Job, JobInput, JobOutput, run};
use crossfade_testkit::clock::ManualClock;
use crossfade_testkit::memory::MemoryIo;
use std::io::SeekFrom;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const STEREO_48K: OutputFormat = OutputFormat {
    sample_rate: 48_000,
    channels: 2,
};

/// Every sample different, so a frame out of place shows: a ramp per channel plus noise.
fn pcm(frames: usize, channels: usize) -> Vec<u8> {
    let mut x = 0x2545_f491_4f6c_dd1du64;
    (0..frames * channels)
        .flat_map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let ramp = ((i / channels) as i64 * 7 + (i % channels) as i64 * 5000) % 20_000;
            let v = (ramp - 10_000 + (x % 512) as i64) as i16;
            v.to_le_bytes()
        })
        .collect()
}

fn wav(rate: u32, layout: ChannelLayout, frames: usize) -> Vec<u8> {
    let channels = usize::from(layout.channels());
    wav_bytes(
        &spec(rate, SampleFormat::S16, layout),
        &pcm(frames, channels),
    )
}

fn io_with(files: &[(&str, Vec<u8>)]) -> MemoryIo {
    let mut io = MemoryIo::new();
    for (path, bytes) in files {
        io.insert(path, bytes.clone());
    }
    io
}

/// `path` in `io` converted to FLAC by the engine at `to`.
fn flac(io: &mut MemoryIo, path: &str, to: &str) {
    let job = Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: path.into() }],
        outputs: vec![JobOutput {
            path: to.into(),
            audio: AudioSettings::default(),
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
}

/// The oracle: the session's frames from `from`, converted to `format` offline in one pass
/// (the standard mix, then the fast resampler, as one chain).
fn expected(bytes: &[u8], from: u64, format: OutputFormat) -> Vec<f32> {
    let mut io = io_with(&[("x", bytes.to_vec())]);
    let mut session = DecodeSession::open("x", &mut io, &Limits::default()).unwrap();
    session.seek(from).unwrap();
    let input = session.stream().audio.clone();
    let layout = device_layout(format.channels).unwrap();
    let mut filters = Vec::new();
    if input.layout() != &layout {
        filters.push(FilterPlan::Mix(MixPlan {
            layout: layout.clone(),
            matrix: None,
        }));
    }
    if input.sample_rate() != format.sample_rate {
        filters.push(FilterPlan::Resample(ResamplePlan {
            rate: format.sample_rate,
            quality: Quality::Fast,
        }));
    }
    let mut out = Vec::new();
    let mut frame = AudioFrame::new(&input);
    let mut converted = AudioFrame::new(&input);
    if filters.is_empty() {
        while session.next_frame(&mut frame).unwrap() {
            convert_format(&frame, SampleFormat::F32, &mut converted).unwrap();
            out.extend(f32s(&converted));
        }
        return out;
    }
    let plan = ChainPlan {
        filters,
        dither: Dither::None,
    };
    let mut chain = Chain::new(&input, &plan, SampleFormat::F32).unwrap();
    while session.next_frame(&mut frame).unwrap() {
        chain.send_frame(&frame).unwrap();
        while chain.receive_frame(&mut converted).unwrap() {
            out.extend(f32s(&converted));
        }
    }
    chain.drain();
    while chain.receive_frame(&mut converted).unwrap() {
        out.extend(f32s(&converted));
    }
    out
}

fn f32s(frame: &AudioFrame) -> Vec<f32> {
    match &frame.samples {
        Samples::F32(s) => s.clone(),
        other => panic!("{other:?}"),
    }
}

struct Rig {
    player: Player,
    out: VirtualOutput,
    notified: Arc<AtomicUsize>,
}

fn rig(io: impl JobIo + Send + 'static, format: OutputFormat) -> Rig {
    let (sink, out) = VirtualSink::new(format);
    let notified = Arc::new(AtomicUsize::new(0));
    let n = Arc::clone(&notified);
    let player = Player::new(Box::new(sink), Box::new(io), Limits::default(), move || {
        n.fetch_add(1, Ordering::SeqCst);
    })
    .unwrap();
    Rig {
        player,
        out,
        notified,
    }
}

/// A rig with `path` loaded and its buffer full.
fn loaded(bytes: Vec<u8>, format: OutputFormat) -> Rig {
    let mut r = rig(io_with(&[("a.wav", bytes)]), format);
    r.player.load("a.wav");
    r.player.settle();
    r
}

fn stereo(frames: usize) -> Vec<u8> {
    wav(48_000, ChannelLayout::stereo(), frames)
}

/// Pulls `frames` once the worker has filled the buffer.
fn pull(r: &Rig, frames: usize) -> Vec<f32> {
    r.player.settle();
    r.out.pull(frames)
}

fn frames(samples: &[f32], channels: usize, range: std::ops::Range<usize>) -> &[f32] {
    &samples[range.start * channels..range.end * channels]
}

#[test]
fn the_virtual_sink_receives_frames_in_order_on_a_manual_clock() {
    let bytes = stereo(48_000);
    let r = loaded(bytes.clone(), STEREO_48K);
    assert_eq!(r.player.state(), State::Ready);
    assert_eq!(r.player.duration(), Some(48_000));
    let mut r = r;
    r.player.play();
    assert_eq!(r.player.state(), State::Playing);
    let clock = ManualClock::new();
    let mut got = Vec::new();
    for _ in 0..50 {
        r.player.settle();
        clock.advance(Duration::from_millis(10));
        got.extend(r.out.run_until(&clock));
    }
    // Half a second at 48 kHz, in device blocks, exactly the source's first half second.
    assert_eq!(got.len(), 24_000 * 2);
    assert_eq!(got, expected(&bytes, 0, STEREO_48K)[..24_000 * 2]);
    assert_eq!(r.player.position(), 24_000);
    assert_eq!(r.player.underruns(), 0);
}

#[test]
fn a_flac_file_plays_its_decoded_samples() {
    let mut io = io_with(&[("a.wav", wav(48_000, ChannelLayout::stereo(), 20_000))]);
    flac(&mut io, "a.wav", "a.flac");
    let bytes = io.get("a.flac").unwrap();
    let mut r = rig(io, STEREO_48K);
    r.player.load("a.flac");
    r.player.settle();
    r.player.play();
    let mut got = Vec::new();
    for _ in 0..20 {
        got.extend(pull(&r, 1000));
    }
    assert_eq!(got, expected(&bytes, 0, STEREO_48K));
}

#[test]
fn play_pause_and_toggle() {
    let mut r = loaded(stereo(48_000), STEREO_48K);
    assert!(!r.out.is_running());
    assert!(r.out.pull(100).is_empty(), "a stopped sink pulls nothing");
    r.player.toggle();
    assert_eq!(r.player.state(), State::Playing);
    assert!(r.out.is_running());
    assert_eq!(pull(&r, 1000).len(), 2000);
    r.player.pause();
    assert_eq!(r.player.state(), State::Paused);
    assert!(r.out.pull(1000).is_empty());
    assert_eq!(r.player.position(), 1000);
    r.player.toggle();
    assert_eq!(r.player.state(), State::Playing);
    r.player.toggle();
    assert_eq!(r.player.state(), State::Paused);
    r.player.play();
    pull(&r, 500);
    assert_eq!(r.player.position(), 1500);
}

#[test]
fn the_position_counts_frames_pulled_not_frames_decoded() {
    let mut r = loaded(stereo(48_000), STEREO_48K);
    r.player.play();
    // The worker has decoded half a second ahead; only what was pulled counts.
    assert_eq!(r.player.position(), 0);
    for k in [1, 511, 4096] {
        let before = r.player.position();
        pull(&r, k);
        assert_eq!(r.player.position(), before + k as u64);
    }
}

#[test]
fn a_seek_while_playing_plays_the_sessions_frame_there_first() {
    let bytes = stereo(48_000);
    let mut r = loaded(bytes.clone(), STEREO_48K);
    r.player.play();
    pull(&r, 3000);
    r.player.seek(31_337);
    assert_eq!(r.player.position(), 31_337, "the target at once");
    assert_eq!(r.player.state(), State::Playing);
    let got = pull(&r, 2000);
    assert_eq!(got, expected(&bytes, 31_337, STEREO_48K)[..4000]);
    assert_eq!(r.player.position(), 33_337);
}

#[test]
fn a_seek_while_paused_moves_the_position_and_stays_paused() {
    let bytes = stereo(48_000);
    let mut r = loaded(bytes.clone(), STEREO_48K);
    r.player.play();
    pull(&r, 1000);
    r.player.pause();
    r.player.seek(12_000);
    assert_eq!(r.player.state(), State::Paused);
    assert_eq!(r.player.position(), 12_000);
    r.player.settle();
    assert_eq!(r.player.position(), 12_000);
    assert!(r.out.pull(10).is_empty());
    r.player.play();
    assert_eq!(pull(&r, 100), expected(&bytes, 12_000, STEREO_48K)[..200]);
}

#[test]
fn seeks_to_the_start_the_end_and_past_it() {
    let bytes = stereo(10_000);
    let mut r = loaded(bytes.clone(), STEREO_48K);
    r.player.play();
    pull(&r, 4000);
    r.player.seek(0);
    assert_eq!(pull(&r, 10), expected(&bytes, 0, STEREO_48K)[..20]);

    r.player.seek(10_000);
    assert_eq!(r.player.position(), 10_000);
    let rest = pull(&r, 64);
    assert!(rest.iter().all(|s| *s == 0.0));
    assert_eq!(r.player.state(), State::Ended);
    assert_eq!(r.player.position(), 10_000);

    r.player.seek(99_999);
    assert_eq!(r.player.position(), 10_000, "held to the duration");
    assert_eq!(
        r.player.state(),
        State::Paused,
        "a seek after the end waits"
    );
    r.player.seek(9_990);
    r.player.play();
    let tail = pull(&r, 64);
    assert_eq!(frames(&tail, 2, 0..10), expected(&bytes, 9_990, STEREO_48K));
    assert!(frames(&tail, 2, 10..64).iter().all(|s| *s == 0.0));
    assert_eq!(r.player.state(), State::Ended);
}

#[test]
fn the_end_drains_the_buffer_then_ends_at_the_duration() {
    let bytes = stereo(5000);
    let mut r = loaded(bytes.clone(), STEREO_48K);
    let before = r.notified.load(Ordering::SeqCst);
    r.player.play();
    let mut got = Vec::new();
    for _ in 0..6 {
        got.extend(pull(&r, 1000));
    }
    assert_eq!(frames(&got, 2, 0..5000), expected(&bytes, 0, STEREO_48K));
    assert!(frames(&got, 2, 5000..6000).iter().all(|s| *s == 0.0));
    assert_eq!(r.player.state(), State::Ended);
    assert_eq!(r.player.position(), 5000);
    assert_eq!(r.player.underruns(), 0);
    r.player.settle();
    assert!(
        r.notified.load(Ordering::SeqCst) > before,
        "the UI hears of the end"
    );
    // Play again starts over.
    r.player.play();
    assert_eq!(r.player.state(), State::Playing);
    assert_eq!(r.player.position(), 0);
    assert_eq!(pull(&r, 10), expected(&bytes, 0, STEREO_48K)[..20]);
}

#[test]
fn stop_goes_back_to_the_start() {
    let mut r = loaded(stereo(10_000), STEREO_48K);
    r.player.play();
    pull(&r, 2000);
    r.player.stop();
    assert_eq!(r.player.state(), State::Ready);
    assert_eq!(r.player.position(), 0);
    assert!(!r.out.is_running());
}

#[test]
fn scrubbing_is_silent_and_release_resumes_at_the_pointer() {
    let bytes = stereo(48_000);
    let mut r = loaded(bytes.clone(), STEREO_48K);
    r.player.play();
    pull(&r, 1000);
    r.player.scrub_to(20_000);
    assert!(r.player.is_scrubbing());
    assert_eq!(r.player.position(), 20_000);
    assert!(r.out.pull(100).is_empty(), "nothing plays during the drag");
    r.player.scrub_to(30_000);
    r.player.scrub_to(25_000);
    assert_eq!(r.player.position(), 25_000);
    r.player.end_scrub();
    assert!(!r.player.is_scrubbing());
    assert_eq!(r.player.state(), State::Playing);
    assert_eq!(pull(&r, 50), expected(&bytes, 25_000, STEREO_48K)[..100]);

    // A drag while paused leaves it paused at the release point.
    r.player.pause();
    r.player.scrub_to(5_000);
    r.player.end_scrub();
    assert_eq!(r.player.state(), State::Paused);
    assert_eq!(r.player.position(), 5_000);
}

/// Says it has reached the gate, then waits to be let through; taken by the first read there.
type Gate = Arc<Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>>;

/// Hands out the bytes of one file up to `at`, then waits for `release` before reading on.
struct GatedIo {
    inner: MemoryIo,
    at: u64,
    gate: Gate,
}

struct GatedSource {
    inner: Box<dyn MediaSource>,
    pos: u64,
    at: u64,
    gate: Gate,
}

impl MediaSource for GatedSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos < self.at {
            let n = buf.len().min((self.at - self.pos) as usize);
            let got = self.inner.read(&mut buf[..n])?;
            self.pos += got as u64;
            return Ok(got);
        }
        // Only a read that reaches the gate in order waits (opening seeks to look for
        // trailing chunks, which is not playback reading on).
        let gate = (self.pos == self.at)
            .then(|| self.gate.lock().unwrap().take())
            .flatten();
        if let Some((reached, release)) = gate {
            reached.send(()).unwrap();
            let _ = release.recv();
        }
        let got = self.inner.read(buf)?;
        self.pos += got as u64;
        Ok(got)
    }
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.pos = self.inner.seek(pos)?;
        Ok(self.pos)
    }
    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }
}

impl JobIo for GatedIo {
    fn open_source(&mut self, path: &str) -> Result<Box<dyn MediaSource>, Error> {
        Ok(Box::new(GatedSource {
            inner: self.inner.open_source(path)?,
            pos: 0,
            at: self.at,
            gate: Arc::clone(&self.gate),
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

const GATE_BYTES: u64 = 44 + 4 * 40_960;

#[test]
fn an_underrun_is_silence_counted_and_playback_recovers_without_losing_frames() {
    let bytes = stereo(48_000);
    let (reached, at_gate) = mpsc::channel();
    let (release_tx, released) = mpsc::channel();
    let io = GatedIo {
        inner: io_with(&[("a.wav", bytes.clone())]),
        // Ten whole packets of 4096 frames, past what opening the file reads ahead to sniff
        // it, then the read blocks.
        at: GATE_BYTES,
        gate: Arc::new(Mutex::new(Some((reached, released)))),
    };
    let mut r = rig(io, STEREO_48K);
    // Declared after the rig so it drops first: a failing assertion must not leave the worker
    // stuck at the gate while the player waits for it.
    let release = release_tx;
    r.player.load("a.wav");
    r.player.play();
    let want = expected(&bytes, 0, STEREO_48K);
    // The buffer holds half a second; once 20,000 frames are out the worker refills up to the
    // gate and sticks there, with 20,960 frames buffered.
    assert_eq!(pull(&r, 20_000), frames(&want, 2, 0..20_000));
    at_gate.recv().unwrap();
    let got = r.out.pull(24_000);
    let have = got
        .chunks(2)
        .position(|f| f.iter().all(|s| *s == 0.0))
        .expect("silence where the decoder could not keep up");
    assert_eq!(have, 20_960, "the frames before the gate");
    assert_eq!(frames(&got, 2, 0..have), frames(&want, 2, 20_000..40_960));
    assert!(frames(&got, 2, have..24_000).iter().all(|s| *s == 0.0));
    assert_eq!(r.player.underruns(), 1);
    assert_eq!(
        r.player.position(),
        40_960,
        "silence does not move the position"
    );
    assert_eq!(r.player.state(), State::Playing);

    release.send(()).unwrap();
    let next = pull(&r, 1000);
    assert_eq!(
        next,
        frames(&want, 2, 40_960..41_960),
        "nothing was skipped"
    );
    assert_eq!(r.player.position(), 41_960);
    assert_eq!(r.player.underruns(), 1);
}

#[test]
fn loading_another_file_while_playing_replaces_it_stopped_at_zero() {
    let a = stereo(48_000);
    let b = wav(48_000, ChannelLayout::stereo(), 9_000);
    let mut r = rig(io_with(&[("a.wav", a), ("b.wav", b.clone())]), STEREO_48K);
    r.player.load("a.wav");
    r.player.settle();
    r.player.play();
    pull(&r, 4000);
    r.player.load("b.wav");
    assert_eq!(r.player.state(), State::Loading);
    assert_eq!(r.player.position(), 0);
    assert!(!r.out.is_running());
    r.player.settle();
    assert_eq!(r.player.state(), State::Ready);
    assert_eq!(r.player.duration(), Some(9_000));
    r.player.play();
    assert_eq!(pull(&r, 100), expected(&b, 0, STEREO_48K)[..200]);
}

#[test]
fn a_file_that_will_not_open_fails_as_a_state() {
    let mut r = rig(
        io_with(&[("junk.wav", b"not audio at all".to_vec())]),
        STEREO_48K,
    );
    let before = r.notified.load(Ordering::SeqCst);
    r.player.load("junk.wav");
    r.player.settle();
    assert!(
        matches!(r.player.state(), State::Failed(ref why) if !why.is_empty()),
        "{:?}",
        r.player.state()
    );
    assert!(r.notified.load(Ordering::SeqCst) > before);
    r.player.play();
    assert!(matches!(r.player.state(), State::Failed(_)));
    assert!(!r.out.is_running());
    r.player.load("missing.wav");
    r.player.settle();
    assert!(matches!(r.player.state(), State::Failed(_)));
}

#[test]
fn nothing_loaded_is_empty_and_ignores_the_transport() {
    let mut r = rig(MemoryIo::new(), STEREO_48K);
    assert_eq!(r.player.state(), State::Empty);
    r.player.play();
    r.player.seek(100);
    assert_eq!(r.player.state(), State::Empty);
    assert_eq!(r.player.position(), 0);
    assert_eq!(r.player.duration(), None);
}

#[test]
fn another_rate_and_layout_converts_and_the_position_maps_back_exactly() {
    // Mono 16 kHz into a stereo 48 kHz device: three device frames per source frame.
    let bytes = wav(16_000, ChannelLayout::mono(), 16_000);
    let mut r = loaded(bytes.clone(), STEREO_48K);
    r.player.play();
    let want = expected(&bytes, 0, STEREO_48K);
    let got = pull(&r, 3000);
    assert_eq!(got, want[..6000]);
    assert!(got.chunks(2).all(|f| f[0] == f[1]), "mono to both sides");
    assert_eq!(r.player.position(), 1000);
    pull(&r, 1);
    assert_eq!(
        r.player.position(),
        1000,
        "a third of a source frame is not one yet"
    );
    pull(&r, 2);
    assert_eq!(r.player.position(), 1001);

    r.player.seek(8_000);
    let after = pull(&r, 300);
    assert_eq!(after, expected(&bytes, 8_000, STEREO_48K)[..600]);
    assert_eq!(r.player.position(), 8_100);

    // To the end: the position lands on the duration.
    let mut n = 0;
    while r.player.state() == State::Playing && n < 100 {
        pull(&r, 4096);
        n += 1;
    }
    assert_eq!(r.player.state(), State::Ended);
    assert_eq!(r.player.position(), 16_000);
}

#[test]
fn more_device_channels_than_the_file_leaves_the_extras_silent() {
    let quad = OutputFormat {
        sample_rate: 48_000,
        channels: 4,
    };
    let bytes = stereo(4000);
    let mut r = loaded(bytes.clone(), quad);
    r.player.play();
    let got = pull(&r, 1000);
    assert_eq!(got.len(), 4000);
    let source = expected(&bytes, 0, STEREO_48K);
    for (i, f) in got.chunks(4).enumerate() {
        assert_eq!(&f[..2], &source[i * 2..i * 2 + 2]);
        assert_eq!(&f[2..], &[0.0, 0.0]);
    }
}

#[test]
fn a_surround_file_is_mixed_down_for_a_stereo_device() {
    let bytes = wav(48_000, ChannelLayout::from_name("5.1").unwrap(), 2000);
    let mut r = loaded(bytes.clone(), STEREO_48K);
    r.player.play();
    assert_eq!(pull(&r, 2000), expected(&bytes, 0, STEREO_48K));
}

#[test]
fn dropping_the_player_stops_the_sink_and_the_worker() {
    let r = loaded(stereo(48_000), STEREO_48K);
    let Rig {
        mut player, out, ..
    } = r;
    player.play();
    assert!(out.is_running());
    drop(player);
    assert!(!out.is_running());
    assert!(out.pull(10).is_empty(), "the feed went with it");
}

#[test]
fn a_sink_that_refuses_the_feed_fails_the_player() {
    struct Refuses;
    impl AudioSink for Refuses {
        fn format(&self) -> OutputFormat {
            STEREO_48K
        }
        fn connect(&mut self, _: crossfade_app::player::Feed) -> Result<(), String> {
            Err("no device".into())
        }
        fn start(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn pause(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn stop(&mut self) {}
    }
    let made = Player::new(
        Box::new(Refuses),
        Box::new(MemoryIo::new()),
        Limits::default(),
        || {},
    );
    assert_eq!(made.err().as_deref(), Some("no device"));
}

#[test]
fn the_readout_is_short() {
    assert_eq!(clock_text(0, 48_000), "0:00.0");
    assert_eq!(clock_text(83 * 48_000 + 4_800 * 4, 48_000), "1:23.4");
    assert_eq!(clock_text(3_725 * 44_100, 44_100), "1:02:05.0");
    assert_eq!(
        transport_text(83 * 10 + 4, Some(296 * 10 + 7), 10),
        "1:23.4 / 4:56.7"
    );
    assert_eq!(transport_text(5, None, 10), "0:00.5");
}

#[test]
fn seeks_made_while_nothing_pulls_play_the_last_target_first() {
    let bytes = stereo(48_000);
    let mut r = loaded(bytes.clone(), STEREO_48K);
    for frame in [1000, 2000, 3000, 4000, 5000, 6000] {
        r.player.seek(frame);
    }
    r.player.play();
    assert_eq!(pull(&r, 100), expected(&bytes, 6000, STEREO_48K)[..200]);
}
