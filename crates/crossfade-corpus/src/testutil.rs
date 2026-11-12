//! Shared helpers for unit tests: temp dirs and entries built from bytes.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

use crate::{Entry, Expect, Source};

pub(crate) fn sha_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A fresh, empty directory unique to `tag` and this process.
pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("crossfade-corpus-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(crate) fn entry(id: &str, name: &str, data: &[u8]) -> Entry {
    Entry {
        id: id.into(),
        name: name.into(),
        source: Source::Url(format!("https://example.org/{name}")),
        bytes: data.len() as u64,
        sha256: sha_hex(data),
        licence: "CC0-1.0".into(),
        expect: Expect::Decode,
        md5: None,
        note: None,
    }
}
