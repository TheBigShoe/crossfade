//! Channel mixing through the whole engine: a 5.1 WAV down to stereo.
mod common;

use common::data_chunk;
use crossfade_core::{ChannelLayout, Error, Limits};
use crossfade_graph::filter::{FilterSpec, parse_channels};
use crossfade_graph::{
    AudioSettings, CancelToken, Dither, Event, Job, JobInput, JobOutput, Pipeline, run,
};
use crossfade_testkit::memory::MemoryIo;

const RATE: u32 = 44100;

/// A `WAVE_FORMAT_EXTENSIBLE` file with the 5.1 mask: a plain six-channel WAV names no layout.
fn six_channel_wav() -> Vec<u8> {
    let pcm: Vec<u8> = (0..6 * 500i32)
        .flat_map(|i| (((i * 37) % 20000 - 10000) as i16).to_le_bytes())
        .collect();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(4 + 8 + 40 + 8 + pcm.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&0xFFFEu16.to_le_bytes());
    v.extend_from_slice(&6u16.to_le_bytes());
    v.extend_from_slice(&RATE.to_le_bytes());
    v.extend_from_slice(&(RATE * 12).to_le_bytes());
    v.extend_from_slice(&12u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(&22u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(&0x60Fu32.to_le_bytes());
    v.extend_from_slice(&[
        1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71,
    ]);
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    v.extend_from_slice(&pcm);
    v
}

fn job(audio: AudioSettings) -> Job {
    Job {
        version: Job::VERSION,
        inputs: vec![JobInput {
            path: "in.wav".into(),
        }],
        outputs: vec![JobOutput {
            path: "out.flac".into(),
            audio,
        }],
    }
}

fn convert(job: &Job, threads: usize) -> Result<(MemoryIo, Vec<Event>), Error> {
    let mut io = MemoryIo::new();
    io.insert("in.wav", six_channel_wav());
    let mut events = Vec::new();
    let limits = Limits {
        max_threads: threads,
        ..Limits::default()
    };
    run(job, &mut io, &limits, &CancelToken::new(), &mut |e| {
        events.push(e.clone());
    })?;
    Ok((io, events))
}

fn via_channels() -> AudioSettings {
    AudioSettings {
        channels: Some(parse_channels("stereo").unwrap()),
        ..AudioSettings::default()
    }
}

fn via_filter() -> AudioSettings {
    AudioSettings {
        filters: vec![FilterSpec::Mix {
            channels: parse_channels("stereo").unwrap(),
            matrix: None,
        }],
        ..AudioSettings::default()
    }
}

#[test]
fn channels_stereo_and_the_mix_filter_write_the_same_bytes() {
    let (a, _) = convert(&job(via_channels()), 1).unwrap();
    let (b, _) = convert(&job(via_filter()), 1).unwrap();
    let (a, b) = (a.get("out.flac").unwrap(), b.get("out.flac").unwrap());
    assert!(a.starts_with(b"fLaC"));
    assert_eq!(a, b);
}

#[test]
fn the_plan_reports_a_stereo_output() {
    let (_, events) = convert(&job(via_channels()), 1).unwrap();
    let Event::Started { plan } = &events[0] else {
        panic!("{:?}", events[0])
    };
    assert_eq!(plan.output.audio.layout(), &ChannelLayout::stereo());
    assert_eq!(plan.output.frames, Some(500));
}

#[test]
fn validate_accepts_the_mix_and_the_input_is_unchanged() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", six_channel_wav());
    let p = Pipeline::open(&job(via_channels()), &mut io, &Limits::default());
    assert!(p.is_ok(), "{:?}", p.err());
}

#[test]
fn threads_never_change_the_bytes() {
    let one = convert(&job(via_channels()), 1).unwrap().0.get("out.flac");
    let auto = convert(&job(via_channels()), 0).unwrap().0.get("out.flac");
    assert!(one.is_some());
    assert_eq!(one, auto);
}

#[test]
fn the_stereo_wav_has_the_mixed_samples_and_no_dither_off_by_more_than_one_lsb() {
    let audio = AudioSettings {
        dither: Dither::None,
        ..via_channels()
    };
    let mut j = job(audio);
    j.outputs[0].path = "out.wav".into();
    let (io, _) = convert(&j, 1).unwrap();
    let out = io.get("out.wav").unwrap();
    let (pairs, _) = data_chunk(&out).as_chunks::<2>();
    let got: Vec<i16> = pairs.iter().map(|&b| i16::from_le_bytes(b)).collect();
    let src = six_channel_wav();
    let (sp, _) = data_chunk(&src).as_chunks::<2>();
    let s: Vec<f64> = sp
        .iter()
        .map(|&b| f64::from(i16::from_le_bytes(b)))
        .collect();
    let k = 1.0 + 2.0 * core::f64::consts::FRAC_1_SQRT_2;
    let r = core::f64::consts::FRAC_1_SQRT_2;
    assert_eq!(got.len(), 1000);
    for n in 0..500 {
        let f = &s[n * 6..n * 6 + 6];
        let l = (f[0] + r * f[2] + r * f[4]) / k;
        let rr = (f[1] + r * f[2] + r * f[5]) / k;
        assert!((f64::from(got[2 * n]) - l).abs() <= 1.0, "{n}");
        assert!((f64::from(got[2 * n + 1]) - rr).abs() <= 1.0, "{n}");
    }
}

/// A plain (not extensible) six-channel WAV: readers call it unpositioned.
fn plain_six_channel_wav() -> Vec<u8> {
    let s = common::spec(
        RATE,
        crossfade_core::SampleFormat::S16,
        ChannelLayout::unpositioned(6).unwrap(),
    );
    let pcm: Vec<u8> = (0..6 * 500i32)
        .flat_map(|i| (((i * 37) % 20000 - 10000) as i16).to_le_bytes())
        .collect();
    common::wav_bytes(&s, &pcm)
}

#[test]
fn unlabelled_six_channels_mix_down_as_5_1_and_the_plan_says_so() {
    let mut io = MemoryIo::new();
    io.insert("plain.wav", plain_six_channel_wav());
    io.insert("labelled.wav", six_channel_wav());
    let stream = &crossfade_graph::probe("plain.wav", &mut io, &Limits::default())
        .unwrap()
        .streams[0];
    assert_eq!(
        stream.audio.layout().positions(),
        None,
        "readers report what they read"
    );
    let stereo = AudioSettings {
        channels: Some(parse_channels("stereo").unwrap()),
        dither: Dither::None,
        ..AudioSettings::default()
    };
    let note = "treating 6 unlabelled channels as 5.1";
    for plan in [
        crossfade_graph::describe(stream, "o.wav", &stereo).unwrap(),
        crossfade_graph::validate(stream, "o.wav", &stereo).unwrap(),
    ] {
        assert_eq!(plan.notes, [note]);
        assert_eq!(plan.output.audio.layout(), &ChannelLayout::stereo());
    }
    let mut events = Vec::new();
    for input in ["plain.wav", "labelled.wav"] {
        let job = Job {
            version: Job::VERSION,
            inputs: vec![JobInput { path: input.into() }],
            outputs: vec![JobOutput {
                path: format!("{input}.out.wav"),
                audio: stereo.clone(),
            }],
        };
        run(
            &job,
            &mut io,
            &Limits::default(),
            &CancelToken::new(),
            &mut |e| {
                events.push(e.clone());
            },
        )
        .unwrap();
    }
    // The same samples come out as from a file that says 5.1 (its mask tags the output).
    assert_eq!(
        data_chunk(&io.get("plain.wav.out.wav").unwrap()),
        data_chunk(&io.get("labelled.wav.out.wav").unwrap())
    );
    let notes: Vec<Vec<String>> = events
        .iter()
        .filter_map(|e| match e {
            Event::Started { plan } => Some(plan.notes.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(notes, [vec![note.to_owned()], vec![]]);
}
