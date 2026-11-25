//! The hand-written header stays honest: the functions it declares are the ones the crate
//! exports (one list, `EXPORTS`, checked against the sources too), and its constants have
//! the crate's values. No C compiler or binding generator is needed.

use crossfade_ffi::*;
use std::collections::BTreeSet;
use std::path::Path;

fn read(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap()
}

/// The text without `/* */` and `//` comments, which may name functions in passing.
fn code(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, r)| r);
        } else if let Some(after) = rest.strip_prefix("//") {
            rest = after.split_once('\n').map_or("", |(_, r)| r);
            out.push('\n');
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Every `cf_name` followed by `(`: what a C declaration (or Rust definition) looks like.
fn called_names(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let starts = bytes[i..].starts_with(&['c', 'f', '_']) && (i == 0 || !ident(bytes[i - 1]));
        if starts {
            let mut j = i;
            while j < bytes.len() && ident(bytes[j]) {
                j += 1;
            }
            let mut k = j;
            while k < bytes.len() && bytes[k].is_whitespace() {
                k += 1;
            }
            if bytes.get(k) == Some(&'(') {
                names.insert(bytes[i..j].iter().collect());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    names
}

fn exports() -> BTreeSet<String> {
    EXPORTS.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn the_header_declares_exactly_the_exported_functions() {
    let header = code(&read("include/crossfade.h"));
    assert_eq!(called_names(&header), exports());
    assert_eq!(EXPORTS.len(), exports().len(), "a name is listed twice");
}

#[test]
fn the_list_is_every_extern_function_in_the_sources() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/abi");
    let mut defined = BTreeSet::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in text.lines() {
            if let Some((_, rest)) = line.split_once("extern \"C\" fn ") {
                defined.insert(rest.split('(').next().unwrap().to_owned());
            }
        }
    }
    assert_eq!(defined, exports());
}

/// `#define NAME value` lines.
fn defines(header: &str) -> Vec<(String, String)> {
    header
        .lines()
        .filter_map(|l| l.trim().strip_prefix("#define "))
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            Some((parts.next()?.to_owned(), parts.next()?.to_owned()))
        })
        .collect()
}

#[test]
fn the_headers_constants_are_the_crates() {
    let header = code(&read("include/crossfade.h"));
    let got: Vec<(String, String)> = defines(&header)
        .into_iter()
        .filter(|(name, _)| name.starts_with("CF_"))
        .collect();
    let want: Vec<(String, String)> = [
        ("CF_ABI_VERSION", cf_abi_version() as i32),
        ("CF_OK", CF_OK),
        ("CF_ERR_NULL_ARGUMENT", CF_ERR_NULL_ARGUMENT),
        ("CF_ERR_INVALID_UTF8", CF_ERR_INVALID_UTF8),
        ("CF_ERR_INVALID_JSON", CF_ERR_INVALID_JSON),
        ("CF_ERR_REFUSED", CF_ERR_REFUSED),
        ("CF_ERR_NOT_FOUND", CF_ERR_NOT_FOUND),
        ("CF_ERR_WRONG_THREAD", CF_ERR_WRONG_THREAD),
        ("CF_ERR_FAILED", CF_ERR_FAILED),
        ("CF_ERR_PANIC", CF_ERR_PANIC),
    ]
    .into_iter()
    .map(|(n, v)| (n.to_owned(), v.to_string()))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn the_header_is_plain_c_with_a_guard_and_extern_c() {
    let header = read("include/crossfade.h");
    assert!(header.contains("#ifndef CROSSFADE_H"));
    assert!(header.contains("#include <stdint.h>"));
    assert!(header.contains("#include <stddef.h>"));
    assert!(header.contains("extern \"C\" {"));
    for name in EXPORTS {
        // Each function has its own doc comment just above it.
        let at = header.find(&format!("{name}(")).unwrap();
        let before = header[..at].trim_end();
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let decl_start = header[..line_start].trim_end();
        assert!(decl_start.ends_with("*/"), "{name} has no doc comment");
    }
}
