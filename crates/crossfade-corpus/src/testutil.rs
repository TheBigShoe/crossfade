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

/// A gzip-compressed tar holding `(name, type, data)` items, written without
/// the builder's path checks so tests can make hostile archives.
pub(crate) fn tar_gz_items(items: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (name, ty, data) in items {
        let mut h = tar::Header::new_gnu();
        h.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name.as_bytes());
        h.set_entry_type(*ty);
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        if matches!(ty, tar::EntryType::Symlink | tar::EntryType::Link) {
            h.set_link_name("target").unwrap();
        }
        h.set_cksum();
        b.append(&h, *data).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

/// A tar.gz of regular files.
pub(crate) fn tar_gz(members: &[(&str, &[u8])]) -> Vec<u8> {
    let items: Vec<_> = members
        .iter()
        .map(|(n, d)| (*n, tar::EntryType::Regular, *d))
        .collect();
    tar_gz_items(&items)
}

/// A manifest with one archive at `https://example.org/<id>.tar.gz` holding
/// `archive` and one entry per `(member path, data)` that is expected to be in
/// it; ids are `opus/rfc8251/<file name>`.
pub(crate) fn archive_manifest(archive: &[u8], members: &[(&str, &[u8])]) -> crate::Manifest {
    let mut text = format!(
        "[[archive]]\nid = \"opus/rfc8251\"\nurl = \"{ARCHIVE_URL}\"\nbytes = {}\n\
         sha256 = \"{}\"\nlicence = \"unstated\"\nformat = \"tar.gz\"\n",
        archive.len(),
        sha_hex(archive)
    );
    for (member, data) in members {
        let name = member.rsplit('/').next().unwrap();
        text += &format!(
            "[[file]]\nid = \"opus/rfc8251/{name}\"\nname = \"{name}\"\narchive = \"opus/rfc8251\"\n\
             member = \"{member}\"\nbytes = {}\nsha256 = \"{}\"\nlicence = \"unstated\"\n\
             expect = \"data\"\n",
            data.len(),
            sha_hex(data)
        );
    }
    crate::Manifest::parse(&text).unwrap()
}

pub(crate) const ARCHIVE_URL: &str = "https://example.org/opus/v.tar.gz";
