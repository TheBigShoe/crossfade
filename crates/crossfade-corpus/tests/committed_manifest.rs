//! The committed `corpus.toml` must stay consistent with the pinned suite.

use std::collections::HashSet;

use crossfade_corpus::{Entry, Expect, Manifest, workspace_root};

const PREFIX: &str = "https://raw.githubusercontent.com/ietf-wg-cellar/flac-test-files/\
                      aa7b0c6cf32994c106ae517a08134c28a96ff5b2/";

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn load() -> Manifest {
    Manifest::load(&workspace_root().join("corpus.toml")).unwrap()
}

/// The pinned FLAC suite; the other codecs' groups have their own tests.
fn flac(m: &Manifest) -> Vec<&Entry> {
    m.entries()
        .iter()
        .filter(|e| e.id.starts_with("flac/"))
        .collect()
}

#[test]
fn has_86_entries_split_64_11_11() {
    let m = load();
    let n = |p: &str| m.entries().iter().filter(|e| e.id.starts_with(p)).count();
    assert_eq!(flac(&m).len(), 86);
    assert_eq!(
        (n("flac/subset/"), n("flac/uncommon/"), n("flac/faulty/")),
        (64, 11, 11)
    );
}

#[test]
fn ids_and_names_are_unique_per_directory() {
    let m = load();
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for e in flac(&m) {
        assert!(ids.insert(&e.id), "{}", e.id);
        let dir = e.id.rsplit_once('/').unwrap().0;
        assert!(names.insert((dir, &e.name)), "{}", e.name);
    }
}

#[test]
fn urls_are_pinned_and_decode_to_the_name() {
    for e in flac(&load()) {
        let url = e.url().unwrap();
        let rest = url.strip_prefix(PREFIX).unwrap_or_else(|| panic!("{url}"));
        let dir = e.id.split('/').nth(1).unwrap();
        let path = percent_decode(rest);
        assert_eq!(path, format!("{dir}/{}", e.name), "{}", e.id);
        assert!(!rest.contains(' '));
    }
}

#[test]
fn expectations_follow_the_directory() {
    for e in flac(&load()) {
        let dir = e.id.split('/').nth(1).unwrap();
        match dir {
            "subset" => assert_eq!(e.expect, Expect::Decode, "{}", e.id),
            "faulty" => assert_eq!(e.expect, Expect::NoPanic, "{}", e.id),
            _ => assert_ne!(e.expect, Expect::NoPanic, "{}", e.id),
        }
        if e.expect == Expect::Decode {
            assert!(e.md5.is_some(), "{} needs an md5", e.id);
        }
        assert_eq!(e.licence, "CC0-1.0");
    }
}
