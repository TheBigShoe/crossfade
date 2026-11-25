//! Versions, `about`, strings and errors through the C functions.
// Tests are the foreign caller: every unsafe call passes pointers made just above it.
#![allow(clippy::undocumented_unsafe_blocks)]

mod common;

use common::{json_out, last_error, take};
use crossfade_ffi::*;
use std::ffi::{CStr, c_char};

#[test]
fn the_abi_version_is_one_and_the_version_is_the_crates() {
    assert_eq!(cf_abi_version(), 1);
    let version = unsafe { CStr::from_ptr(cf_version()) }.to_str().unwrap();
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn about_is_the_app_layers_lines_with_the_versions() {
    let about = json_out(|out| unsafe { cf_about(out) });
    let lines: Vec<String> = crossfade_app::about::lines().into();
    assert_eq!(about["lines"], serde_json::json!(lines));
    assert_eq!(about["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(about["abi_version"], 1);
}

#[test]
fn freeing_null_does_nothing() {
    unsafe { cf_string_free(std::ptr::null_mut()) };
}

#[test]
fn a_null_out_pointer_is_an_error_with_a_message() {
    assert_eq!(
        unsafe { cf_about(std::ptr::null_mut()) },
        CF_ERR_NULL_ARGUMENT
    );
    let message = last_error().unwrap();
    assert!(message.contains("null"), "{message}");
}

#[test]
fn a_successful_call_clears_the_last_error() {
    assert_eq!(
        unsafe { cf_about(std::ptr::null_mut()) },
        CF_ERR_NULL_ARGUMENT
    );
    let mut out: *mut c_char = std::ptr::null_mut();
    assert_eq!(unsafe { cf_about(&mut out) }, CF_OK);
    take(out);
    assert_eq!(last_error(), None);
}

#[test]
fn the_last_error_belongs_to_its_thread() {
    assert_eq!(
        unsafe { cf_about(std::ptr::null_mut()) },
        CF_ERR_NULL_ARGUMENT
    );
    let other = std::thread::spawn(last_error).join().unwrap();
    assert_eq!(other, None);
    assert!(last_error().is_some());
}

#[test]
fn a_panic_inside_a_call_comes_back_as_an_error() {
    let status = guard(|| panic!("boom"));
    assert_eq!(status, CF_ERR_PANIC);
    let message = last_error().unwrap();
    assert!(message.contains("boom"), "{message}");
    // The thread carries on as normal.
    assert_eq!(guard(|| Ok(())), CF_OK);
}

#[test]
fn a_failure_keeps_its_code_and_words() {
    let status = guard(|| Err(Failure::refused("That file already exists.")));
    assert_eq!(status, CF_ERR_REFUSED);
    assert_eq!(last_error().as_deref(), Some("That file already exists."));
}
