#![no_main]
//! The filter text parser's promise: whatever the text, it never panics, and whatever it
//! accepts parses back from its `Display` to the same value and survives the job's JSON.
//! The value-word parsers the shortcut flags use never panic either.
//!
//! A first byte with the top bit set prefixes a filter name (picked by the low bits), so every
//! filter's grammar is reached without a dictionary; otherwise the bytes are the whole text.

use crossfade_graph::filter::{
    FilterSpec, parse_channels, parse_db, parse_dbtp, parse_lufs, parse_rate, parse_time,
};
use libfuzzer_sys::fuzz_target;

const NAMES: [&str; 6] = ["gain", "trim", "fade", "normalize", "resample", "mix"];

fuzz_target!(|data: &[u8]| {
    let (text, rest) = match data.split_first() {
        Some((&b, rest)) if b & 0x80 != 0 => {
            let Ok(rest) = std::str::from_utf8(rest) else { return };
            (format!("{} {rest}", NAMES[usize::from(b & 0x7F) % NAMES.len()]), rest)
        }
        _ => {
            let Ok(text) = std::str::from_utf8(data) else { return };
            (text.to_owned(), text)
        }
    };
    if let Ok(spec) = text.parse::<FilterSpec>() {
        let shown = spec.to_string();
        assert_eq!(
            shown.parse::<FilterSpec>().as_ref(),
            Ok(&spec),
            "{text:?} shown as {shown:?}"
        );
        let json = serde_json::to_string(&spec).unwrap();
        assert_eq!(serde_json::from_str::<FilterSpec>(&json).unwrap(), spec, "{json}");
    }
    let _ = (parse_db(rest), parse_time(rest), parse_lufs(rest), parse_dbtp(rest));
    let _ = (parse_rate(rest), parse_channels(rest));
});
