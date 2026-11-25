//! Background analysis through the C functions gives what the app layer gives directly.
#![allow(clippy::undocumented_unsafe_blocks)]

mod common;

use common::{Dir, c, fixture, json_out, write_wav};
use crossfade_app::FsIo;
use crossfade_app::analysis::{Analysis, analyze};
use crossfade_app::waveview::{WaveView, readout};
use crossfade_core::Limits;
use crossfade_ffi::*;
use crossfade_graph::CancelToken;
use serde_json::Value;

struct Analyser(*mut CfAnalysis);

impl Analyser {
    fn start(path: &str) -> Self {
        let path = c(path);
        let mut h = std::ptr::null_mut();
        assert_eq!(unsafe { cf_analysis_new(path.as_ptr(), &mut h) }, CF_OK);
        Self(h)
    }

    fn state(&self) -> Value {
        json_out(|out| unsafe { cf_analysis_state(self.0, out) })
    }

    fn finish(&self) -> Value {
        loop {
            let state = self.state();
            if state["state"] != "running" {
                return state;
            }
            let at = state["revision"].as_u64().unwrap();
            let mut now = 0;
            assert_eq!(
                unsafe { cf_analysis_wait(self.0, at, 60_000, &mut now) },
                CF_OK
            );
            assert!(now > at, "a minute passed with no change");
        }
    }

    /// `n` columns of `lane`, `None` where nothing is analysed.
    fn columns(&self, lane: u32, start: u64, fpc: u64, n: usize) -> Vec<Option<(f32, f32)>> {
        let mut mins = vec![0.0f32; n];
        let mut maxs = vec![0.0f32; n];
        let mut written = usize::MAX;
        let status = unsafe {
            cf_analysis_columns(
                self.0,
                lane,
                start,
                fpc,
                mins.as_mut_ptr(),
                maxs.as_mut_ptr(),
                n,
                &mut written,
            )
        };
        assert_eq!(status, CF_OK);
        (0..n)
            .map(|i| {
                if i < written {
                    Some((mins[i], maxs[i]))
                } else {
                    assert!(mins[i].is_nan() && maxs[i].is_nan());
                    None
                }
            })
            .collect()
    }
}

impl Drop for Analyser {
    fn drop(&mut self) {
        unsafe { cf_analysis_destroy(self.0) };
    }
}

fn direct(path: &str) -> Analysis {
    analyze(path, &mut FsIo::new(), &Limits::default(), &CancelToken::new())
}

fn same_as_direct(path: &str, width: u32) {
    let a = Analyser::start(path);
    let state = a.finish();
    let want = direct(path);
    assert_eq!(state["state"], "done", "{path}");
    let l = &want.loudness;
    assert_eq!(state["loudness"]["integrated"].as_f64(), l.integrated, "{path}");
    assert_eq!(state["loudness"]["range"].as_f64(), l.range, "{path}");
    assert_eq!(state["loudness"]["true_peak"].as_f64(), l.true_peak, "{path}");
    assert_eq!(state["loudness"]["sample_peak"].as_f64(), Some(l.sample_peak));
    let snapshot = crossfade_app::analysis::Snapshot {
        version: want.version,
        stream: want.stream.clone(),
        frames_done: want.frames_done,
        state: want.state.clone(),
        loudness: want.loudness.clone(),
    };
    assert_eq!(state["readout"]["text"], readout(&snapshot).text());

    let stream = want.stream.as_ref().unwrap();
    let frames = stream.frames.unwrap_or(want.frames_done);
    assert_eq!(state["frames"].as_u64(), Some(frames));
    assert_eq!(state["sample_rate"], stream.audio.sample_rate());
    let mut view = WaveView::new();
    view.set_source(frames, stream.audio.sample_rate());
    view.set_width(width);
    let lanes = view.draw_columns(&want.pyramid);
    assert_eq!(state["lanes"].as_u64(), Some(lanes.len() as u64), "{path}");
    for (lane, expected) in lanes.iter().enumerate() {
        let got = a.columns(
            lane as u32,
            view.start(),
            view.frames_per_pixel(),
            view.columns(),
        );
        assert_eq!(&got, expected, "{path} lane {lane}");
    }
    // A zoomed range, past the end too.
    let fpc = (view.frames_per_pixel() / 4).max(1);
    let start = frames / 2;
    let n = width as usize;
    assert_eq!(a.columns(0, start, fpc, n), {
        let mut v = want.pyramid.columns(0, start, fpc, n);
        if lanes.len() == 1 && want.pyramid.channels() > 2 {
            v = want.pyramid.columns_union(start, fpc, n);
        }
        v
    });
}

#[test]
fn stereo_gives_two_lanes_of_the_same_columns_and_loudness() {
    let dir = Dir::new("analysis-stereo");
    let path = write_wav(&dir, "a.wav", 48_000, 2, 48_000 * 5);
    same_as_direct(&path, 300);
}

#[test]
fn surround_and_compressed_files_match_too() {
    same_as_direct(&fixture("wav/f32-5.1.wav"), 64);
    same_as_direct(&fixture("flac/stereo-24bit.flac"), 200);
    same_as_direct(&fixture("mp3/cbr128.mp3"), 120);
}

#[test]
fn an_unreadable_file_ends_failed_with_the_readout() {
    let dir = Dir::new("analysis-bad");
    let junk = dir.path("junk.wav");
    std::fs::write(&junk, b"nope").unwrap();
    let a = Analyser::start(&junk);
    let state = a.finish();
    assert_eq!(state["state"], "failed");
    assert!(
        state["readout"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Could not analyse: ")
    );
    assert!(state["failure"].is_string());
    assert_eq!(state["lanes"], 0);
}

#[test]
fn buffers_are_checked() {
    let dir = Dir::new("analysis-buffers");
    let path = write_wav(&dir, "a.wav", 8_000, 1, 8_000);
    let a = Analyser::start(&path);
    a.finish();
    let mut written = 9;
    // Nothing asked for: null buffers are fine.
    let status = unsafe {
        cf_analysis_columns(
            a.0,
            0,
            0,
            256,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut written,
        )
    };
    assert_eq!(status, CF_OK);
    assert_eq!(written, 0);
    let mut one = [0.0f32];
    let status = unsafe {
        cf_analysis_columns(
            a.0,
            0,
            0,
            256,
            std::ptr::null_mut(),
            one.as_mut_ptr(),
            1,
            &mut written,
        )
    };
    assert_eq!(status, CF_ERR_NULL_ARGUMENT);
    // A lane the file does not have.
    let (mut lo, mut hi) = ([0.0f32], [0.0f32]);
    let status = unsafe {
        cf_analysis_columns(a.0, 1, 0, 256, lo.as_mut_ptr(), hi.as_mut_ptr(), 1, &mut written)
    };
    assert_eq!(status, CF_ERR_REFUSED);
    // Zero frames per column reads as one, as the pyramid takes it.
    assert_eq!(a.columns(0, 0, 0, 2), a.columns(0, 0, 1, 2));
}

#[test]
fn cancelling_stops_the_pass_and_null_is_an_error() {
    let dir = Dir::new("analysis-cancel");
    let path = write_wav(&dir, "a.wav", 48_000, 2, 48_000 * 60);
    let a = Analyser::start(&path);
    assert_eq!(unsafe { cf_analysis_cancel(a.0) }, CF_OK);
    let state = a.finish();
    assert!(
        state["state"] == "cancelled" || state["state"] == "done",
        "{state}"
    );
    assert_eq!(
        unsafe { cf_analysis_cancel(std::ptr::null()) },
        CF_ERR_NULL_ARGUMENT
    );
    unsafe { cf_analysis_destroy(std::ptr::null_mut()) };
}
