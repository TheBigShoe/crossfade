//! Filters through the whole engine: WAV in, the chain, FLAC or WAV out, read back.
mod common;

use common::{data_chunk, spec, wav_bytes};
use crossfade_core::{ChannelLayout, Error, JobIo, Limits, SampleFormat};
use crossfade_dsp::level::db_to_linear;
use crossfade_graph::filter::FilterSpec;
use crossfade_graph::{
    AudioSettings, CancelToken, Dither, Event, Job, JobInput, JobOutput, Pipeline, run,
};
use crossfade_testkit::memory::MemoryIo;

const RATE: u32 = 44100;

fn s16_wav(layout: ChannelLayout, samples: &[i16]) -> Vec<u8> {
    let pcm: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    wav_bytes(&spec(RATE, SampleFormat::S16, layout), &pcm)
}

fn s16_of(wav: &[u8]) -> Vec<i16> {
    let (pairs, _) = data_chunk(wav).as_chunks::<2>();
    pairs.iter().map(|&b| i16::from_le_bytes(b)).collect()
}

fn gain(db: f64) -> FilterSpec {
    FilterSpec::Gain { db }
}

fn job_with(input: &str, output: &str, audio: AudioSettings) -> Job {
    Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: input.into() }],
        outputs: vec![JobOutput {
            path: output.into(),
            audio,
        }],
    }
}

fn filters(filters: Vec<FilterSpec>, dither: Dither) -> AudioSettings {
    AudioSettings {
        filters,
        dither,
        ..AudioSettings::default()
    }
}

fn convert(io: &mut dyn JobIo, job: &Job, threads: usize) -> Result<Vec<Event>, Error> {
    let mut events = Vec::new();
    let limits = Limits {
        max_threads: threads,
        ..Limits::default()
    };
    run(job, io, &limits, &CancelToken::new(), &mut |e| {
        events.push(e.clone());
    })?;
    Ok(events)
}

/// The output as 16-bit samples, through the engine's own FLAC decoder when it is a FLAC.
fn read_back(io: &mut MemoryIo, path: &str) -> Vec<i16> {
    if path.ends_with(".flac") {
        let back = format!("{path}.wav");
        convert(io, &job_with(path, &back, AudioSettings::default()), 1).unwrap();
        s16_of(&io.get(&back).unwrap())
    } else {
        s16_of(&io.get(path).unwrap())
    }
}

/// Round half away from zero, as the project's conversion rule does.
fn expected(s: i16, linear: f64) -> i16 {
    (f64::from(s) * linear).round().clamp(-32768.0, 32767.0) as i16
}

fn ramp(n: usize) -> Vec<i16> {
    (0..n)
        .map(|i| (((i * 2741) % 60_000) as i32 - 30_000) as i16)
        .collect()
}

#[test]
fn minus_six_db_without_dither_is_exact_through_flac() {
    let x = ramp(20_000);
    let mut io = MemoryIo::new();
    io.insert("in.wav", s16_wav(ChannelLayout::stereo(), &x));
    let job = job_with(
        "in.wav",
        "out.flac",
        filters(vec![gain(-6.0)], Dither::None),
    );
    convert(&mut io, &job, 1).unwrap();
    let k = db_to_linear(-6.0);
    let want: Vec<i16> = x.iter().map(|&s| expected(s, k)).collect();
    assert_eq!(read_back(&mut io, "out.flac"), want);
}

#[test]
fn tpdf_dither_on_a_constant_is_two_lsb_around_the_value_and_repeats_exactly() {
    let x = vec![1000i16; 100_000];
    let mut outs = Vec::new();
    for _ in 0..2 {
        let mut io = MemoryIo::new();
        io.insert("in.wav", s16_wav(ChannelLayout::mono(), &x));
        let job = job_with("in.wav", "out.wav", filters(vec![gain(0.0)], Dither::Tpdf));
        convert(&mut io, &job, 1).unwrap();
        outs.push(io.get("out.wav").unwrap());
    }
    assert_eq!(outs[0], outs[1], "same input, same bytes");
    let v = s16_of(&outs[0]);
    let (lo, hi) = (*v.iter().min().unwrap(), *v.iter().max().unwrap());
    assert_eq!((lo, hi), (999, 1001));
    let mean = v.iter().map(|&s| f64::from(s)).sum::<f64>() / v.len() as f64;
    assert!((mean - 1000.0).abs() < 0.01, "mean {mean}");
}

#[test]
fn the_dither_error_does_not_follow_the_signal() {
    let x: Vec<i16> = (0..100_000)
        .map(|i| (8000.0 * libm::sin(f64::from(i) * 0.0123)) as i16)
        .collect();
    let mut io = MemoryIo::new();
    io.insert("in.wav", s16_wav(ChannelLayout::mono(), &x));
    let job = job_with(
        "in.wav",
        "out.wav",
        filters(vec![gain(-20.0)], Dither::Tpdf),
    );
    convert(&mut io, &job, 1).unwrap();
    let k = db_to_linear(-20.0);
    let ideal: Vec<f64> = x.iter().map(|&s| f64::from(s) * k).collect();
    let err: Vec<f64> = s16_of(&io.get("out.wav").unwrap())
        .iter()
        .zip(&ideal)
        .map(|(&o, &i)| f64::from(o) - i)
        .collect();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let (me, mi) = (mean(&err), mean(&ideal));
    let cov = err
        .iter()
        .zip(&ideal)
        .map(|(e, i)| (e - me) * (i - mi))
        .sum::<f64>();
    let var = |v: &[f64], m: f64| v.iter().map(|x| (x - m) * (x - m)).sum::<f64>();
    let corr = cov / (var(&err, me) * var(&ideal, mi)).sqrt();
    assert!(me.abs() < 0.01, "mean error {me} LSB");
    assert!(corr.abs() < 0.01, "correlation {corr}");
}

#[test]
fn twenty_db_up_clips_and_says_how_many() {
    let x = ramp(10_000);
    let mut io = MemoryIo::new();
    io.insert("in.wav", s16_wav(ChannelLayout::stereo(), &x));
    let job = job_with("in.wav", "out.wav", filters(vec![gain(20.0)], Dither::Tpdf));
    let events = convert(&mut io, &job, 1).unwrap();
    let want = x
        .iter()
        .filter(|&&s| {
            let v = (f64::from(s) * 10.0).round();
            !(-32768.0..=32767.0).contains(&v)
        })
        .count() as u64;
    assert!(want > 8000, "{want}");
    let n = events.len();
    assert_eq!(events[n - 2], Event::Clipped { samples: want });
    assert!(matches!(events[n - 1], Event::Finished { .. }));
    let v = s16_of(&io.get("out.wav").unwrap());
    assert!(v.contains(&32767) && v.contains(&-32768));
}

#[test]
fn no_clip_event_when_nothing_clipped() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", s16_wav(ChannelLayout::mono(), &ramp(1000)));
    let job = job_with("in.wav", "out.wav", filters(vec![gain(-1.0)], Dither::Tpdf));
    let events = convert(&mut io, &job, 1).unwrap();
    assert!(!events.iter().any(|e| matches!(e, Event::Clipped { .. })));
}

#[test]
fn float_output_carries_the_samples_untouched() {
    let s = spec(RATE, SampleFormat::F32, ChannelLayout::stereo());
    let samples: Vec<f32> = (0..4000).map(|i| ((i as f32) * 0.01).sin() * 1.7).collect();
    let pcm: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut io = MemoryIo::new();
    io.insert("in.wav", wav_bytes(&s, &pcm));
    let job = job_with("in.wav", "out.wav", filters(vec![gain(0.0)], Dither::Tpdf));
    let events = convert(&mut io, &job, 1).unwrap();
    assert_eq!(data_chunk(&io.get("out.wav").unwrap()), pcm.as_slice());
    assert!(
        !events.iter().any(|e| matches!(e, Event::Clipped { .. })),
        "1.7 is not clipped in float"
    );
}

#[test]
fn threads_never_change_the_bytes() {
    let mut outs = Vec::new();
    for threads in [1, 0, 3] {
        let mut io = MemoryIo::new();
        io.insert("in.wav", s16_wav(ChannelLayout::stereo(), &ramp(200_000)));
        let job = job_with(
            "in.wav",
            "out.flac",
            filters(vec![gain(-3.0)], Dither::Tpdf),
        );
        convert(&mut io, &job, threads).unwrap();
        outs.push(io.get("out.flac").unwrap());
    }
    assert_eq!(outs[0], outs[1], "1 thread against auto");
    assert_eq!(outs[0], outs[2], "1 thread against 3");
}

#[test]
fn asking_for_the_rate_and_layout_the_input_has_changes_no_byte() {
    let x = ramp(5000);
    let mut plain = MemoryIo::new();
    plain.insert("in.wav", s16_wav(ChannelLayout::stereo(), &x));
    convert(
        &mut plain,
        &job_with("in.wav", "out.flac", AudioSettings::default()),
        1,
    )
    .unwrap();
    let mut same = MemoryIo::new();
    same.insert("in.wav", s16_wav(ChannelLayout::stereo(), &x));
    let audio = AudioSettings {
        sample_rate: Some(RATE),
        channels: Some(crossfade_graph::filter::parse_channels("2").unwrap()),
        dither: Dither::None,
        ..AudioSettings::default()
    };
    convert(&mut same, &job_with("in.wav", "out.flac", audio), 1).unwrap();
    assert_eq!(plain.get("out.flac"), same.get("out.flac"));
}

#[test]
fn a_filter_not_built_yet_stops_the_job_before_anything_is_written() {
    for (audio, name) in [
        (
            AudioSettings {
                sample_rate: Some(48000),
                ..AudioSettings::default()
            },
            "resample",
        ),
        (
            AudioSettings {
                channels: Some(crossfade_graph::filter::parse_channels("mono").unwrap()),
                ..AudioSettings::default()
            },
            "mix",
        ),
    ] {
        let mut io = MemoryIo::new();
        io.insert("in.wav", s16_wav(ChannelLayout::stereo(), &ramp(100)));
        let err = convert(&mut io, &job_with("in.wav", "out.flac", audio), 1).unwrap_err();
        assert_eq!(
            err,
            Error::Unsupported {
                what: format!("the {name} filter is not built yet")
            }
        );
        assert_eq!(io.get("out.flac"), None);
        assert!(!io.has_pending());
    }
}

#[test]
fn stream_copy_refuses_anything_that_changes_the_audio() {
    let copy = |audio: AudioSettings| AudioSettings {
        codec: Some("copy".into()),
        ..audio
    };
    for (audio, why) in [
        (
            copy(filters(vec![gain(-1.0)], Dither::Tpdf)),
            "--codec copy cannot apply filters",
        ),
        (
            copy(AudioSettings {
                sample_rate: Some(48000),
                ..AudioSettings::default()
            }),
            "--codec copy cannot change the sample rate",
        ),
        (
            copy(AudioSettings {
                channels: Some(crossfade_graph::filter::parse_channels("mono").unwrap()),
                ..AudioSettings::default()
            }),
            "--codec copy cannot change the channels",
        ),
    ] {
        let mut io = MemoryIo::new();
        io.insert("in.wav", s16_wav(ChannelLayout::stereo(), &ramp(100)));
        let err = convert(&mut io, &job_with("in.wav", "out.wav", audio), 1).unwrap_err();
        assert!(err.to_string().contains(why), "{err}");
    }
}

#[test]
fn several_inputs_are_for_concat_which_is_not_built_yet() {
    let mut io = MemoryIo::new();
    io.insert("a.wav", s16_wav(ChannelLayout::stereo(), &ramp(100)));
    let mut job = job_with("a.wav", "out.wav", AudioSettings::default());
    job.inputs.push(JobInput {
        path: "a.wav".into(),
    });
    let err = Pipeline::open(&job, &mut io, &Limits::default())
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        "unsupported: joining 2 inputs (crossfade concat) is not built yet"
    );
}

#[test]
fn the_plan_reports_the_processed_output() {
    let mut io = MemoryIo::new();
    io.insert("in.wav", s16_wav(ChannelLayout::stereo(), &ramp(1000)));
    let job = job_with(
        "in.wav",
        "out.flac",
        AudioSettings {
            sample_format: Some(SampleFormat::S24),
            ..filters(vec![gain(-1.0)], Dither::Tpdf)
        },
    );
    let events = convert(&mut io, &job, 1).unwrap();
    let Event::Started { plan } = &events[0] else {
        panic!("{:?}", events[0])
    };
    assert_eq!(plan.output.audio.format(), SampleFormat::S24);
    assert_eq!(plan.output.bits_per_raw_sample, None);
    assert_eq!(plan.output.frames, Some(500));
}
