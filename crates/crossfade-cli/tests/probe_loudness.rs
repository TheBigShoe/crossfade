//! `crossfade probe --loudness`, through the library entry the binary uses.
use crossfade_cli::{Outcome, run_command};
use crossfade_graph::CancelToken;
use crossfade_testkit::memory::MemoryIo;

/// 16-bit stereo 48 kHz WAV: a 1 kHz sine at `dbfs` on both channels.
fn tone_wav(dbfs: f64, seconds: u32) -> Vec<u8> {
    let rate = 48_000u32;
    let amp = 10f64.powf(dbfs / 20.0) * 32767.0;
    let mut pcm = Vec::new();
    for i in 0..rate * seconds {
        let s = (amp * (2.0 * std::f64::consts::PI * 1000.0 * f64::from(i) / f64::from(rate)).sin())
            .round() as i16;
        pcm.extend_from_slice(&s.to_le_bytes());
        pcm.extend_from_slice(&s.to_le_bytes());
    }
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    v.extend_from_slice(&pcm);
    v
}

fn probe(wav: Vec<u8>, args: &[&str]) -> String {
    let mut io = MemoryIo::new();
    io.insert("a.wav", wav);
    match run_command(
        args.iter().copied(),
        &mut io,
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap()
    {
        Outcome::Done(s) => s,
        other => panic!("{other:?}"),
    }
}

#[test]
fn text_adds_one_loudness_line_after_the_streams() {
    let out = probe(tone_wav(-23.0, 20), &["probe", "a.wav", "--loudness"]);
    insta::assert_snapshot!(out, @"
    Input: wav, 3840044 bytes
      Stream 0: audio, pcm_s16le, 48000 Hz, stereo, s16, 20.000 s (960000 frames)
    Loudness: -23.0 LUFS integrated, 0.0 LU range, -23.0 dBTP true peak
    ");
}

#[test]
fn json_carries_a_loudness_object() {
    let out = probe(
        tone_wav(-23.0, 20),
        &["probe", "a.wav", "--loudness", "--json"],
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["schema"], 1);
    let l = &v["loudness"];
    assert!(
        (l["integrated_lufs"].as_f64().unwrap() + 23.0).abs() < 0.1,
        "{out}"
    );
    assert!(l["range_lu"].is_number() && l["true_peak_dbtp"].is_number());
    assert!((l["sample_peak_dbfs"].as_f64().unwrap() + 23.0).abs() < 0.01);
    let at = |k: &str| out.find(&format!("\"{k}\"")).unwrap();
    assert!(
        at("integrated_lufs") < at("range_lu")
            && at("range_lu") < at("true_peak_dbtp")
            && at("true_peak_dbtp") < at("sample_peak_dbfs"),
        "{out}"
    );
}

#[test]
fn without_the_flag_nothing_changes() {
    let wav = tone_wav(-23.0, 1);
    let json = probe(wav.clone(), &["probe", "a.wav", "--json"]);
    assert!(!json.contains("loudness"), "{json}");
    assert!(!probe(wav, &["probe", "a.wav"]).contains("Loudness"));
}

#[test]
fn silence_says_so_in_text_and_nulls_in_json() {
    let wav = tone_wav(-120.0, 2); // rounds to digital silence
    let text = probe(wav.clone(), &["probe", "a.wav", "--loudness"]);
    assert!(
        text.ends_with("Loudness: no loudness (silence or under 400 ms)\n"),
        "{text}"
    );
    let json = probe(wav, &["probe", "a.wav", "--loudness", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v["loudness"]["integrated_lufs"].is_null());
    assert!(v["loudness"]["sample_peak_dbfs"].is_null());
}

#[test]
fn a_cancelled_loudness_pass_is_a_cancelled_outcome() {
    let mut io = MemoryIo::new();
    io.insert("a.wav", tone_wav(-23.0, 2));
    let cancel = CancelToken::new();
    cancel.cancel();
    let out = run_command(
        ["probe", "a.wav", "--loudness"],
        &mut io,
        &cancel,
        &mut |_| {},
    )
    .unwrap();
    assert!(matches!(out, Outcome::Cancelled), "{out:?}");
}
