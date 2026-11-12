//! The committed `corpus.toml` must stay consistent with the pinned suite.

use std::collections::HashSet;

use crossfade_corpus::{Entry, Expect, Manifest, Source, workspace_root};

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

fn group<'a>(m: &'a Manifest, prefix: &str) -> Vec<&'a Entry> {
    m.entries()
        .iter()
        .filter(|e| e.id.starts_with(prefix))
        .collect()
}

#[test]
fn the_other_codecs_have_the_expected_file_counts() {
    let m = load();
    let ext = |prefix: &str, ext: &str| {
        group(&m, prefix)
            .iter()
            .filter(|e| e.name.ends_with(ext))
            .count()
    };
    assert_eq!(group(&m, "opus/rfc8251/").len(), 36);
    assert_eq!(ext("opus/rfc8251/", ".bit"), 12);
    assert_eq!(group(&m, "vorbis/xiph/").len(), 29);
    assert_eq!(group(&m, "mp3/iso/").len(), 84);
    assert_eq!((ext("mp3/iso/", ".mpg"), ext("mp3/iso/", ".wav")), (40, 44));
    assert_eq!(group(&m, "aac/iso/").len(), 62);
    assert_eq!(
        [".mp4", ".wav", ".adts", ".adif"].map(|e| ext("aac/iso/", e)),
        [28, 20, 7, 7]
    );
    assert_eq!(m.entries().len(), 86 + 36 + 29 + 84 + 62);
}

#[test]
fn new_groups_are_reference_data_with_their_licence_and_a_matching_name() {
    let m = load();
    for e in m.entries().iter().filter(|e| !e.id.starts_with("flac/")) {
        assert_eq!(e.expect, Expect::Data, "{}", e.id);
        let parts: Vec<_> = e.id.split('/').collect();
        assert_eq!(parts.len(), 3, "{}", e.id);
        assert_eq!(parts[2], e.name, "{}", e.id);
        assert!(e.md5.is_none());
        let want = match parts[0] {
            "opus" => "unstated (opus-codec.org test vectors); fetched, never redistributed",
            "vorbis" => "unstated (xiph.org test vectors); fetched, never redistributed",
            "mp3" | "aac" => {
                "ISO electronic insert: use for the standard's conformance purposes, no redistribution"
            }
            other => panic!("unknown codec {other}"),
        };
        assert_eq!(e.licence, want, "{}", e.id);
    }
}

#[test]
fn opus_members_come_from_the_one_archive() {
    let m = load();
    let a = m.archive("opus/rfc8251").unwrap();
    assert_eq!(a.bytes, 74_624_664);
    assert_eq!(
        a.sha256,
        "6b26a22f9ba87b2b836906a9bb7afec5f8e54d49553b1200382520ee6fedfa55"
    );
    assert_eq!(
        a.url,
        "https://opus-codec.org/static/testvectors/opus_testvectors-rfc8251.tar.gz"
    );
    assert_eq!(m.archives().len(), 1);
    for e in group(&m, "opus/") {
        assert_eq!(
            e.source,
            Source::Archive {
                archive: "opus/rfc8251".into(),
                member: format!("opus_newvectors/{}", e.name)
            },
            "{}",
            e.id
        );
    }
}

#[test]
fn urls_of_the_single_file_groups_point_at_their_hosts() {
    let m = load();
    let iso = "https://standards.iso.org/ittf/PubliclyAvailableStandards/\
               ISO_IEC_14496-26_2010_Bitstreams/";
    let xiph = "https://people.xiph.org/~xiphmont/test-vectors/vorbis/";
    for e in group(&m, "vorbis/") {
        assert_eq!(e.url().unwrap(), format!("{xiph}{}", e.name), "{}", e.id);
    }
    for e in group(&m, "mp3/").into_iter().chain(group(&m, "aac/")) {
        let url = e.url().unwrap();
        let rest = url.strip_prefix(iso).unwrap_or_else(|| panic!("{url}"));
        assert!(rest.starts_with("DVD"), "{url}");
        assert_eq!(
            percent_decode(rest.rsplit('/').next().unwrap()),
            e.name,
            "{url}"
        );
        assert!(!url.contains(' ') && !url.contains('+'), "{url}");
    }
}

#[test]
fn no_entry_is_empty() {
    for e in load().entries() {
        assert!(e.bytes > 0, "{}", e.id);
    }
}
