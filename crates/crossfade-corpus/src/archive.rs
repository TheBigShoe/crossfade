//! Extracting the manifest's members from a verified archive.
//!
//! Safe by construction: what is written, and where, comes from the manifest
//! (the cache path of each entry); the archive only supplies bytes, which are
//! then checked against the entry's own size and SHA-256. Nothing else in the
//! archive is ever written.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use tar::EntryType;

use crate::cache::verify_file;
use crate::{Archive, Cache, Entry, Source};

/// A name that cannot leave the directory it is read for.
fn is_safe_archive_name(name: &str) -> bool {
    let name = name.strip_suffix('/').unwrap_or(name);
    !name.starts_with('/') && !name.contains('\0') && name.split('/').all(|c| c != "..")
}

/// Extract `wanted` (all members of `archive`) from `archive_file`, which the
/// caller has verified. One result per entry, in order. An archive that holds
/// an unsafe path, a link or a device entry, or cannot be read, is refused as
/// a whole: no member of it is installed.
pub(crate) fn extract(
    archive_file: &Path,
    archive: &Archive,
    cache: &Cache,
    wanted: &[&Entry],
) -> Vec<Result<(), String>> {
    let mut results: Vec<Option<Result<(), String>>> = vec![None; wanted.len()];
    let mut staged: Vec<Option<PathBuf>> = vec![None; wanted.len()];
    let scanned = scan(
        archive_file,
        archive,
        cache,
        wanted,
        &mut results,
        &mut staged,
    );
    if let Err(why) = scanned {
        for tmp in staged.iter().flatten() {
            let _ = std::fs::remove_file(tmp);
        }
        return wanted
            .iter()
            .map(|_| Err(format!("archive {}: {why}", archive.id)))
            .collect();
    }
    wanted
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            if let Some(r) = results[i].take() {
                return r;
            }
            match &staged[i] {
                Some(tmp) => std::fs::rename(tmp, cache.path(entry)).map_err(|e| {
                    let _ = std::fs::remove_file(tmp);
                    e.to_string()
                }),
                None => Err(format!(
                    "`{}` is not in the archive {}",
                    member_of(entry),
                    archive.id
                )),
            }
        })
        .collect()
}

fn member_of(entry: &Entry) -> &str {
    match &entry.source {
        Source::Archive { member, .. } => member,
        Source::Url(_) => &entry.name,
    }
}

fn scan(
    archive_file: &Path,
    archive: &Archive,
    cache: &Cache,
    wanted: &[&Entry],
    results: &mut [Option<Result<(), String>>],
    staged: &mut [Option<PathBuf>],
) -> Result<(), String> {
    let file = std::fs::File::open(archive_file).map_err(|e| format!("cannot open: {e}"))?;
    // A gzip bomb costs time, never memory (nothing unwanted is kept), but
    // cap it anyway: real archives here compress far less than 64 to 1.
    let limit = archive.bytes.saturating_mul(64);
    let mut tar = tar::Archive::new(GzDecoder::new(file).take(limit));
    let index: HashMap<&str, usize> = wanted
        .iter()
        .enumerate()
        .rev()
        .map(|(i, e)| (member_of(e), i))
        .collect();
    let mut seen = vec![false; wanted.len()];
    for item in tar.entries().map_err(|e| format!("cannot read: {e}"))? {
        let mut item = item.map_err(|e| format!("cannot read: {e}"))?;
        let ty = item.header().entry_type();
        let name = String::from_utf8_lossy(&item.path_bytes()).into_owned();
        if !is_safe_archive_name(&name) {
            return Err(format!(
                "entry `{name}` has an unsafe path; refusing the archive"
            ));
        }
        if matches!(
            ty,
            EntryType::Symlink
                | EntryType::Link
                | EntryType::Char
                | EntryType::Block
                | EntryType::Fifo
        ) {
            return Err(format!(
                "entry `{name}` is a link or device entry; refusing the archive"
            ));
        }
        let Some(&i) = index.get(name.as_str()) else {
            continue;
        };
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        results[i] = stage_member(cache, wanted[i], &name, ty, &mut item, &mut staged[i]);
    }
    Ok(())
}

/// `Some(error)` when the member cannot be installed; `None` when staged.
fn stage_member(
    cache: &Cache,
    entry: &Entry,
    name: &str,
    ty: EntryType,
    item: &mut impl Read,
    staged: &mut Option<PathBuf>,
) -> Option<Result<(), String>> {
    if !matches!(ty, EntryType::Regular | EntryType::Continuous) {
        return Some(Err(format!("`{name}` is not a regular file")));
    }
    let tmp = cache.temp_path(entry);
    let mut write = || -> Result<(), String> {
        if let Some(parent) = tmp.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        // One byte more than the manifest allows, so an oversize member is
        // noticed without ever being read in full.
        let copied = std::io::copy(
            &mut item.by_ref().take(entry.bytes.saturating_add(1)),
            &mut out,
        )
        .map_err(|e| format!("cannot read `{name}`: {e}"))?;
        if copied > entry.bytes {
            return Err(format!(
                "`{name}` has size above the manifest's {} bytes",
                entry.bytes
            ));
        }
        drop(out);
        verify_file(entry, &tmp).map_err(|p| p.to_string())
    };
    match write() {
        Ok(()) => {
            *staged = Some(tmp);
            None
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Some(Err(e))
        }
    }
}
