//! What every test of the C surface does as a foreign caller would: make C strings, read
//! and free returned ones, look at the last error.
#![allow(dead_code, unreachable_pub)]

use crossfade_ffi::*;
use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};

pub fn c(text: &str) -> CString {
    CString::new(text).unwrap()
}

/// Takes ownership of a string the library returned and frees it the documented way.
pub fn take(ptr: *mut c_char) -> String {
    assert!(!ptr.is_null(), "the library returned no string");
    // SAFETY: `ptr` came from the library just now and is freed once, below.
    let text = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
    // SAFETY: as above; the library allocated it.
    unsafe { cf_string_free(ptr) };
    text
}

/// The last error's message on this thread, or `None`.
pub fn last_error() -> Option<String> {
    let ptr = cf_last_error_message();
    (!ptr.is_null()).then(|| take(ptr))
}

/// A JSON string out of a call shaped `f(&mut out) -> status`.
pub fn json_out(call: impl FnOnce(*mut *mut c_char) -> i32) -> serde_json::Value {
    let mut out: *mut c_char = std::ptr::null_mut();
    let status = call(&mut out);
    assert_eq!(status, CF_OK, "status {status}: {:?}", last_error());
    serde_json::from_str(&take(out)).unwrap()
}

/// A scratch directory removed when dropped.
pub struct Dir(pub PathBuf);

impl Dir {
    pub fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-ffi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    pub fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }

    pub fn listing(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A committed fixture of the format crate's tests.
pub fn fixture(rel: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../crossfade-format/tests/fixtures")
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// A 16-bit WAV of a sine, `frames` long, at `rate` with `channels` channels.
pub fn write_wav(dir: &Dir, name: &str, rate: u32, channels: u16, frames: usize) -> String {
    use crossfade_core::{AudioSpec, ChannelLayout, SampleFormat};
    use crossfade_testkit::{pcm, signal};
    let layout = ChannelLayout::default_for(channels).unwrap();
    let spec = AudioSpec::new(rate, SampleFormat::S16, layout).unwrap();
    let samples = signal::render(
        &signal::Signal::Sine {
            hz: 440.0,
            amplitude: 0.5,
        },
        &spec,
        frames,
    )
    .unwrap();
    let data = pcm::encode_le(&samples, SampleFormat::S16);
    let block = u32::from(channels) * 2;
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * block).to_le_bytes());
    v.extend_from_slice(&(block as u16).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    let path = dir.path(name);
    std::fs::write(&path, v).unwrap();
    path
}
