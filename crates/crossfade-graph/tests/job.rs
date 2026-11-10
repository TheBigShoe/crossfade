use crossfade_core::{Error, SampleFormat};
use crossfade_graph::{AudioSettings, Job, JobInput, JobOutput};

fn job(format: Option<SampleFormat>) -> Job {
    Job {
        version: Job::VERSION,
        inputs: vec![JobInput {
            path: "in.wav".into(),
        }],
        outputs: vec![JobOutput {
            path: "out.wav".into(),
            audio: AudioSettings {
                sample_format: format,
                ..AudioSettings::default()
            },
        }],
    }
}

#[test]
fn json_has_the_pinned_shape() {
    assert_eq!(
        job(Some(SampleFormat::S24)).to_json_pretty(),
        r#"{
  "version": 1,
  "inputs": [
    {
      "path": "in.wav"
    }
  ],
  "outputs": [
    {
      "path": "out.wav",
      "audio": {
        "sample_format": "s24"
      }
    }
  ]
}"#
    );
}

#[test]
fn absent_sample_format_omits_the_key() {
    let json = job(None).to_json_pretty();
    assert!(!json.contains("sample_format"), "{json}");
    assert!(json.contains("\"audio\": {}"), "{json}");
}

#[test]
fn json_round_trips_for_every_format() {
    use SampleFormat::*;
    for f in [
        None,
        Some(U8),
        Some(S16),
        Some(S24),
        Some(S32),
        Some(F32),
        Some(F64),
    ] {
        let j = job(f);
        assert_eq!(Job::from_json(&j.to_json_pretty()), Ok(j));
    }
}

#[test]
fn missing_audio_settings_default_to_none() {
    let j = Job::from_json(
        r#"{"version":1,"inputs":[{"path":"a"}],"outputs":[{"path":"b","audio":{}}]}"#,
    )
    .unwrap();
    assert_eq!(j, job(None).with_paths("a", "b"));
}

trait WithPaths {
    fn with_paths(self, input: &str, output: &str) -> Self;
}

impl WithPaths for Job {
    fn with_paths(mut self, input: &str, output: &str) -> Self {
        self.inputs[0].path = input.into();
        self.outputs[0].path = output.into();
        self
    }
}

#[test]
fn malformed_json_is_malformed() {
    for bad in [
        "",
        "not json",
        "{}",
        r#"{"version":1}"#,
        r#"{"version":"1"}"#,
    ] {
        assert!(
            matches!(Job::from_json(bad), Err(Error::Malformed { .. })),
            "{bad:?}"
        );
    }
}

#[test]
fn unknown_sample_format_is_malformed() {
    let json =
        r#"{"version":1,"inputs":[],"outputs":[{"path":"b","audio":{"sample_format":"s20"}}]}"#;
    assert!(matches!(Job::from_json(json), Err(Error::Malformed { .. })));
}

#[test]
fn other_versions_are_unsupported() {
    for v in [0, 2, 99] {
        let json = format!(r#"{{"version":{v},"inputs":[],"outputs":[]}}"#);
        assert!(
            matches!(Job::from_json(&json), Err(Error::Unsupported { .. })),
            "{v}"
        );
    }
}

#[test]
fn codec_and_compression_level_round_trip_and_keep_old_jobs_unchanged() {
    let audio = AudioSettings {
        sample_format: Some(SampleFormat::S16),
        codec: Some("flac".into()),
        compression_level: Some(8),
    };
    let mut j = job(None);
    j.outputs[0].audio = audio;
    let json = j.to_json_pretty();
    assert!(json.contains("\"codec\": \"flac\""), "{json}");
    assert!(json.contains("\"compression_level\": 8"), "{json}");
    assert_eq!(Job::from_json(&json), Ok(j));
    assert_eq!(Job::VERSION, 1);

    // An M1 job has no new keys, so its bytes are as before.
    let old = job(Some(SampleFormat::S24)).to_json_pretty();
    assert!(
        !old.contains("codec") && !old.contains("compression"),
        "{old}"
    );
}

#[test]
fn a_job_written_before_the_new_fields_still_reads() {
    let j = Job::from_json(
        r#"{"version":1,"inputs":[{"path":"a"}],"outputs":[{"path":"b","audio":{"sample_format":"s16"}}]}"#,
    )
    .unwrap();
    let audio = &j.outputs[0].audio;
    assert_eq!(
        (audio.codec.as_deref(), audio.compression_level),
        (None, None)
    );
}
