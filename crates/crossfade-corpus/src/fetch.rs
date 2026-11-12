//! Getting files into the cache: download or copy to a temp name, verify,
//! then rename, so the final name only ever holds a verified file.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::archive::extract;
use crate::cache::{verify_file, verify_path};
use crate::{Archive, Cache, Entry, Manifest, Source};

/// What the server said about a body, for an error that names the cause.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Received {
    pub content_type: Option<String>,
}

/// Seam for the network, so tests inject a fake.
pub trait Downloader {
    /// Write the body of `url` to `dest`, refusing more than `max_bytes`.
    fn download(&mut self, url: &str, dest: &Path, max_bytes: u64) -> Result<Received, String>;
}

/// Shells out to `curl`, which keeps a TLS stack out of the dependency tree.
#[derive(Debug, Default)]
pub struct CurlDownloader;

impl Downloader for CurlDownloader {
    fn download(&mut self, url: &str, dest: &Path, max_bytes: u64) -> Result<Received, String> {
        let out = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--write-out",
                "%{content_type}",
                "--max-filesize",
            ])
            .arg(max_bytes.to_string())
            .arg("--output")
            .arg(dest)
            .arg(url)
            .output()
            .map_err(|e| format!("cannot run curl: {e}"))?;
        if out.status.success() {
            let ct = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok(Received {
                content_type: (!ct.is_empty()).then_some(ct),
            })
        } else {
            Err(format!(
                "curl: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Skipped,
    Fetched,
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    Starting(&'a Entry),
    /// An archive is being downloaded for its members.
    Archive(&'a Archive),
    Finished(&'a Entry, &'a Outcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Summary {
    pub fetched: usize,
    pub skipped: usize,
    pub failed: Vec<Failure>,
}

impl Summary {
    pub fn is_ok(&self) -> bool {
        self.failed.is_empty()
    }

    fn record(&mut self, entry: &Entry, outcome: &Outcome) {
        match outcome {
            Outcome::Skipped => self.skipped += 1,
            Outcome::Fetched => self.fetched += 1,
            Outcome::Failed(reason) => self.failed.push(Failure {
                id: entry.id.clone(),
                reason: reason.clone(),
            }),
        }
    }
}

/// True when `id` starts with one of the `--only` prefixes (always, with none).
pub fn selected(id: &str, only: &[String]) -> bool {
    only.is_empty() || only.iter().any(|p| id.starts_with(p.as_str()))
}

/// What a wrong download looked like: the ISO server answers a wrong URL with
/// status 200 and an HTML page, which only a size or hash check would catch.
fn received_note(path: &Path, content_type: Option<&str>) -> String {
    let len = std::fs::metadata(path).map_or(0, |m| m.len());
    let mut head = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(48).read_to_end(&mut head);
    }
    let printable = !head.is_empty()
        && head
            .iter()
            .all(|b| b.is_ascii_graphic() || matches!(b, b' ' | b'\n' | b'\r' | b'\t'));
    let start = if printable {
        let text: String = head
            .iter()
            .map(|&b| {
                if b.is_ascii_whitespace() {
                    ' '
                } else {
                    char::from(b)
                }
            })
            .collect();
        format!("`{text}`")
    } else {
        head.iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let lower = String::from_utf8_lossy(&head).to_ascii_lowercase();
    let html = content_type.is_some_and(|c| c.to_ascii_lowercase().contains("html"))
        || lower.trim_start().starts_with("<!doctype html")
        || lower.trim_start().starts_with("<html");
    format!(
        "; received {len} bytes of {}, starting {start}{}",
        content_type.unwrap_or("unknown content type"),
        if html {
            " (an HTML page: the URL is probably wrong)"
        } else {
            ""
        }
    )
}

/// Download `url` to `tmp`, check it and rename it to `dest`; `tmp` never
/// outlives a failure.
fn download_verified(
    downloader: &mut dyn Downloader,
    url: &str,
    (tmp, dest): (&Path, &Path),
    (bytes, sha256): (u64, &str),
) -> Result<(), String> {
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Room for a small error page, so it can be described rather than cut off.
    let max = bytes.saturating_add(1 << 20);
    let result = downloader
        .download(url, tmp, max)
        .and_then(|got| {
            verify_path(tmp, bytes, sha256)
                .map_err(|p| format!("{p}{}", received_note(tmp, got.content_type.as_deref())))
        })
        .and_then(|()| std::fs::rename(tmp, dest).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

fn copy_verified(src: &Path, entry: &Entry, cache: &Cache) -> Result<(), String> {
    let tmp = cache.temp_path(entry);
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let result = std::fs::copy(src, &tmp)
        .map_err(|e| e.to_string())
        .and_then(|_| verify_file(entry, &tmp).map_err(|p| p.to_string()))
        .and_then(|()| std::fs::rename(&tmp, cache.path(entry)).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Download every selected entry that is not already valid. A file is fetched
/// on its own; archive members come from one download of their archive, which
/// is deleted once all its members are in (it is as large as they are). One
/// bad file never stops the rest.
pub fn fetch(
    manifest: &Manifest,
    cache: &Cache,
    downloader: &mut dyn Downloader,
    only: &[String],
    progress: &mut dyn FnMut(Event<'_>),
) -> Summary {
    let mut summary = Summary::default();
    let mut waiting: HashMap<&str, Vec<&Entry>> = HashMap::new();
    for entry in manifest.entries() {
        if !selected(&entry.id, only) {
            continue;
        }
        if cache.verify(entry).is_ok() {
            let outcome = Outcome::Skipped;
            progress(Event::Finished(entry, &outcome));
            summary.record(entry, &outcome);
            continue;
        }
        match &entry.source {
            Source::Url(url) => {
                progress(Event::Starting(entry));
                let outcome = match download_verified(
                    downloader,
                    url,
                    (&cache.temp_path(entry), &cache.path(entry)),
                    (entry.bytes, &entry.sha256),
                ) {
                    Ok(()) => Outcome::Fetched,
                    Err(e) => Outcome::Failed(e),
                };
                progress(Event::Finished(entry, &outcome));
                summary.record(entry, &outcome);
            }
            Source::Archive { archive, .. } => waiting.entry(archive).or_default().push(entry),
        }
    }
    for archive in manifest.archives() {
        let Some(entries) = waiting.remove(archive.id.as_str()) else {
            continue;
        };
        let results = fetch_archive(cache, downloader, archive, &entries, progress);
        let all_ok = results.iter().all(Result::is_ok);
        for (entry, result) in entries.iter().zip(results) {
            let outcome = result.map_or_else(Outcome::Failed, |()| Outcome::Fetched);
            progress(Event::Finished(entry, &outcome));
            summary.record(entry, &outcome);
        }
        if all_ok {
            let _ = std::fs::remove_file(cache.archive_path(archive));
        }
    }
    summary
}

fn fetch_archive(
    cache: &Cache,
    downloader: &mut dyn Downloader,
    archive: &Archive,
    entries: &[&Entry],
    progress: &mut dyn FnMut(Event<'_>),
) -> Vec<Result<(), String>> {
    for entry in entries {
        progress(Event::Starting(entry));
    }
    let file = cache.archive_path(archive);
    if cache.verify_archive(archive).is_err() {
        progress(Event::Archive(archive));
        if let Err(why) = download_verified(
            downloader,
            &archive.url,
            (&cache.temp_archive_path(archive), &file),
            (archive.bytes, &archive.sha256),
        ) {
            return entries
                .iter()
                .map(|_| Err(format!("archive {}: {why}", archive.id)))
                .collect();
        }
    }
    extract(&file, archive, cache, entries)
}

const MAX_DEPTH: usize = 16;

type Index = HashMap<String, Vec<(String, PathBuf)>>;

fn index_tree(dir: &Path, depth: usize, out: &mut Index) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let parent = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for item in rd.flatten() {
        let path = item.path();
        if path.is_dir() {
            if depth < MAX_DEPTH {
                index_tree(&path, depth + 1, out);
            }
        } else if let Some(name) = path.file_name() {
            out.entry(name.to_string_lossy().into_owned())
                .or_default()
                .push((parent.clone(), path));
        }
    }
}

/// `from` may be a directory tree or one file (an archive).
fn build_index(from: &Path) -> Index {
    let mut index = Index::new();
    if from.is_file() {
        if let Some(name) = from.file_name() {
            let parent = from
                .parent()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            index
                .entry(name.to_string_lossy().into_owned())
                .or_default()
                .push((parent, from.to_path_buf()));
        }
    } else {
        index_tree(from, 0, &mut index);
    }
    for files in index.values_mut() {
        files.sort();
    }
    index
}

/// Files called `name`, those in a directory called `group` first.
fn candidates<'a>(index: &'a Index, name: &str, group: &str) -> Vec<&'a Path> {
    let Some(files) = index.get(name) else {
        return vec![];
    };
    let (near, far): (Vec<_>, Vec<_>) = files.iter().partition(|(dir, _)| dir == group);
    near.into_iter()
        .chain(far)
        .map(|(_, p)| p.as_path())
        .collect()
}

fn url_file_name(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

/// Copy files found under `from` into the cache, matching on file name (a file
/// in a directory named like the entry's group, such as `subset`, is tried
/// first) and verify each. Archive members are taken from an already
/// extracted copy, or else extracted from the archive file (found under `from`
/// by its name, or `from` itself). For offline mirrors; entries not found are
/// failures unless the cache already holds them. Nothing is trusted unverified.
pub fn seed(manifest: &Manifest, cache: &Cache, from: &Path, only: &[String]) -> Summary {
    let index = build_index(from);
    let mut summary = Summary::default();
    let mut unextracted: Vec<&Entry> = vec![];
    for entry in manifest.entries() {
        if !selected(&entry.id, only) {
            continue;
        }
        if cache.verify(entry).is_ok() {
            summary.record(entry, &Outcome::Skipped);
            continue;
        }
        let group = entry.id.rsplit('/').nth(1).unwrap_or_default();
        let mut last = None;
        let mut done = false;
        for src in candidates(&index, &entry.name, group) {
            match copy_verified(src, entry, cache) {
                Ok(()) => {
                    done = true;
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        if done {
            summary.record(entry, &Outcome::Fetched);
        } else if matches!(entry.source, Source::Archive { .. }) {
            unextracted.push(entry);
        } else {
            let why = last.unwrap_or_else(|| format!("not found under {}", from.display()));
            summary.record(entry, &Outcome::Failed(why));
        }
    }
    for archive in manifest.archives() {
        let wanted: Vec<&Entry> = unextracted
            .iter()
            .copied()
            .filter(|e| matches!(&e.source, Source::Archive { archive: a, .. } if *a == archive.id))
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let found = candidates(&index, url_file_name(&archive.url), "")
            .into_iter()
            .chain(from.is_file().then_some(from))
            .find(|p| verify_path(p, archive.bytes, &archive.sha256).is_ok());
        let results = match found {
            Some(file) => extract(file, archive, cache, &wanted),
            None => wanted
                .iter()
                .map(|_| {
                    Err(format!(
                        "not found under {}, nor inside a valid copy of {}",
                        from.display(),
                        url_file_name(&archive.url)
                    ))
                })
                .collect(),
        };
        for (entry, result) in wanted.iter().zip(results) {
            summary.record(
                entry,
                &result.map_or_else(Outcome::Failed, |()| Outcome::Fetched),
            );
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{ARCHIVE_URL, archive_manifest, entry, tar_gz, tar_gz_items, temp_dir};
    use crate::{Cache, Manifest};
    use std::collections::HashMap;

    struct Fake {
        bodies: HashMap<String, Result<Vec<u8>, String>>,
        types: HashMap<String, String>,
        calls: Vec<String>,
    }

    impl Downloader for Fake {
        fn download(&mut self, url: &str, dest: &Path, _max: u64) -> Result<Received, String> {
            self.calls.push(url.to_string());
            match self.bodies.get(url) {
                Some(Ok(b)) => {
                    std::fs::write(dest, b).map_err(|e| e.to_string())?;
                    Ok(Received {
                        content_type: self.types.get(url).cloned(),
                    })
                }
                Some(Err(e)) => Err(e.clone()),
                None => Err("404".into()),
            }
        }
    }

    fn manifest(entries: &[&Entry]) -> Manifest {
        let mut text = String::new();
        for e in entries {
            text += &format!(
                "[[file]]\nid = \"{}\"\nname = \"{}\"\nurl = \"{}\"\nbytes = {}\nsha256 = \"{}\"\n\
                 licence = \"CC0-1.0\"\nexpect = \"decode\"\n",
                e.id,
                e.name,
                e.url().unwrap(),
                e.bytes,
                e.sha256
            );
        }
        Manifest::parse(&text).unwrap()
    }

    fn fake(items: &[(&Entry, Result<&[u8], &str>)]) -> Fake {
        Fake {
            bodies: items
                .iter()
                .map(|(e, r)| {
                    (
                        e.url().unwrap().to_string(),
                        r.map(<[u8]>::to_vec).map_err(str::to_string),
                    )
                })
                .collect(),
            types: HashMap::new(),
            calls: vec![],
        }
    }

    fn net_archive(bytes: &[u8]) -> Fake {
        Fake {
            bodies: HashMap::from([(ARCHIVE_URL.to_string(), Ok(bytes.to_vec()))]),
            types: HashMap::new(),
            calls: vec![],
        }
    }

    fn quiet(_: Event<'_>) {}

    #[test]
    fn fetches_and_verifies() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let cache = Cache::new(temp_dir("fetch-ok"));
        let mut dl = fake(&[(&a, Ok(b"aaa"))]);
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, &[], &mut quiet);
        assert_eq!((s.fetched, s.skipped, s.failed.len()), (1, 0, 0));
        assert!(s.is_ok());
        assert_eq!(cache.verify(&a), Ok(()));
        assert!(!cache.temp_path(&a).exists());
    }

    #[test]
    fn skips_files_already_valid() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let cache = Cache::new(temp_dir("fetch-skip"));
        let mut dl = fake(&[(&a, Ok(b"aaa"))]);
        let m = manifest(&[&a]);
        fetch(&m, &cache, &mut dl, &[], &mut quiet);
        let s = fetch(&m, &cache, &mut dl, &[], &mut quiet);
        assert_eq!((s.fetched, s.skipped), (0, 1));
        assert_eq!(dl.calls.len(), 1);
    }

    #[test]
    fn refetches_a_corrupt_cached_file() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let cache = Cache::new(temp_dir("fetch-corrupt"));
        std::fs::create_dir_all(cache.path(&a).parent().unwrap()).unwrap();
        std::fs::write(cache.path(&a), b"bad").unwrap();
        let mut dl = fake(&[(&a, Ok(b"aaa"))]);
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, &[], &mut quiet);
        assert_eq!(s.fetched, 1);
        assert_eq!(cache.verify(&a), Ok(()));
    }

    #[test]
    fn wrong_hash_leaves_nothing_behind_and_is_reported() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let cache = Cache::new(temp_dir("fetch-hash"));
        let mut dl = fake(&[(&a, Ok(b"bbb"))]);
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, &[], &mut quiet);
        assert_eq!(s.fetched, 0);
        assert_eq!(s.failed.len(), 1);
        assert_eq!(s.failed[0].id, "flac/subset/01");
        assert!(
            s.failed[0].reason.contains("sha256"),
            "{}",
            s.failed[0].reason
        );
        assert!(!s.is_ok());
        assert!(!cache.path(&a).exists() && !cache.temp_path(&a).exists());
    }

    #[test]
    fn wrong_size_is_reported() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let cache = Cache::new(temp_dir("fetch-size"));
        let mut dl = fake(&[(&a, Ok(b"aaaa"))]);
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, &[], &mut quiet);
        assert!(s.failed[0].reason.contains("size"));
        assert!(!cache.temp_path(&a).exists());
    }

    #[test]
    fn an_html_page_says_what_was_received() {
        let a = entry("mp3/iso/a.mpg", "a.mpg", b"stream");
        let cache = Cache::new(temp_dir("fetch-html"));
        let page = b"<!DOCTYPE html><html><body>Not found</body></html>";
        let mut dl = fake(&[(&a, Ok(page))]);
        dl.types
            .insert(a.url().unwrap().into(), "text/html; charset=utf-8".into());
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, &[], &mut quiet);
        let why = &s.failed[0].reason;
        assert!(why.contains("text/html"), "{why}");
        assert!(why.contains("<!DOCTYPE html>"), "{why}");
        assert!(why.contains("HTML"), "{why}");
        assert!(
            why.contains("wrong size") || why.contains("sha256"),
            "{why}"
        );
    }

    #[test]
    fn binary_garbage_is_shown_as_hex() {
        let a = entry("mp3/iso/a.mpg", "a.mpg", b"stream");
        let cache = Cache::new(temp_dir("fetch-hex"));
        let mut dl = fake(&[(&a, Ok(&[0xff, 0xfb, 0x90, 0x00, 1, 2, 3, 4]))]);
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, &[], &mut quiet);
        let why = &s.failed[0].reason;
        assert!(why.contains("ff fb 90 00"), "{why}");
        assert!(!why.contains("HTML"), "{why}");
    }

    #[test]
    fn a_failed_download_does_not_stop_the_others() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let b = entry("flac/subset/02", "b.flac", b"bbb");
        let cache = Cache::new(temp_dir("fetch-fail"));
        let mut dl = fake(&[(&a, Err("boom")), (&b, Ok(b"bbb"))]);
        let s = fetch(&manifest(&[&a, &b]), &cache, &mut dl, &[], &mut quiet);
        assert_eq!((s.fetched, s.failed.len()), (1, 1));
        assert!(s.failed[0].reason.contains("boom"));
        assert_eq!(cache.verify(&b), Ok(()));
        assert!(!cache.temp_path(&a).exists());
    }

    #[test]
    fn only_filters_by_id_prefix_and_may_repeat() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let b = entry("flac/faulty/01", "b.flac", b"bbb");
        let c = entry("opus/x/c.bit", "c.bit", b"ccc");
        let cache = Cache::new(temp_dir("fetch-only"));
        let mut dl = fake(&[(&a, Ok(b"aaa")), (&b, Ok(b"bbb")), (&c, Ok(b"ccc"))]);
        let m = manifest(&[&a, &b, &c]);
        let only = ["flac/faulty".to_string()];
        let s = fetch(&m, &cache, &mut dl, &only, &mut quiet);
        assert_eq!(s.fetched, 1);
        assert_eq!(dl.calls, vec![b.url().unwrap().to_string()]);
        let only = ["flac/faulty".to_string(), "opus/".to_string()];
        let s = fetch(&m, &cache, &mut dl, &only, &mut quiet);
        assert_eq!((s.fetched, s.skipped), (1, 1));
        assert!(cache.verify(&a).is_err());
    }

    #[test]
    fn progress_reports_every_file() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let b = entry("flac/subset/02", "b.flac", b"bbb");
        let cache = Cache::new(temp_dir("fetch-progress"));
        let mut dl = fake(&[(&a, Ok(b"aaa")), (&b, Err("x"))]);
        let mut lines = vec![];
        fetch(&manifest(&[&a, &b]), &cache, &mut dl, &[], &mut |ev| {
            if let Event::Finished(e, o) = ev {
                lines.push(format!("{} {o:?}", e.id));
            }
        });
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Fetched") && lines[1].contains("Failed"));
    }

    #[test]
    fn an_archive_is_downloaded_once_and_only_named_members_are_extracted() {
        let tgz = tar_gz(&[
            ("v/a.bit", b"aaa"),
            ("v/unwanted.bit", b"zzzz"),
            ("v/b.dec", b"bbbb"),
        ]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa"), ("v/b.dec", b"bbbb")]);
        let cache = Cache::new(temp_dir("arch-ok"));
        let mut dl = net_archive(&tgz);
        let mut events = vec![];
        let s = fetch(&m, &cache, &mut dl, &[], &mut |ev| {
            events.push(matches!(ev, Event::Archive(_)));
        });
        assert_eq!((s.fetched, s.skipped, s.failed.len()), (2, 0, 0), "{s:?}");
        assert_eq!(dl.calls, vec![ARCHIVE_URL.to_string()]);
        assert!(events.contains(&true));
        for e in m.entries() {
            assert_eq!(cache.verify(e), Ok(()));
        }
        assert!(!cache.dir().join("opus/rfc8251/unwanted.bit").exists());
        assert!(!cache.dir().join("opus/rfc8251/unwanted.bit").exists());
        // The archive is deleted once every member is safely extracted.
        assert!(!cache.archive_path(&m.archives()[0]).exists());
        assert!(!cache.temp_archive_path(&m.archives()[0]).exists());
    }

    #[test]
    fn a_cache_that_holds_every_member_never_touches_the_network() {
        let tgz = tar_gz(&[("v/a.bit", b"aaa")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa")]);
        let cache = Cache::new(temp_dir("arch-skip"));
        let mut dl = net_archive(&tgz);
        fetch(&m, &cache, &mut dl, &[], &mut quiet);
        let s = fetch(&m, &cache, &mut dl, &[], &mut quiet);
        assert_eq!((s.fetched, s.skipped), (0, 1));
        assert_eq!(dl.calls.len(), 1);
    }

    #[test]
    fn only_a_missing_member_is_extracted_and_only_prefixes_gate_the_archive() {
        let tgz = tar_gz(&[("v/a.bit", b"aaa"), ("v/b.dec", b"bbbb")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa"), ("v/b.dec", b"bbbb")]);
        let cache = Cache::new(temp_dir("arch-partial"));
        let mut dl = net_archive(&tgz);
        let only = ["flac/".to_string()];
        let s = fetch(&m, &cache, &mut dl, &only, &mut quiet);
        assert_eq!((s.fetched, dl.calls.len()), (0, 0));
        let only = ["opus/rfc8251/a".to_string()];
        let s = fetch(&m, &cache, &mut dl, &only, &mut quiet);
        assert_eq!(s.fetched, 1);
        assert!(cache.verify(&m.entries()[1]).is_err());
        let s = fetch(&m, &cache, &mut dl, &[], &mut quiet);
        assert_eq!((s.fetched, s.skipped), (1, 1));
    }

    #[test]
    fn a_wrong_archive_is_reported_against_every_member_and_leaves_nothing() {
        let tgz = tar_gz(&[("v/a.bit", b"aaa"), ("v/b.dec", b"bbbb")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa"), ("v/b.dec", b"bbbb")]);
        let cache = Cache::new(temp_dir("arch-bad"));
        let mut dl = net_archive(b"<html>not an archive</html>");
        dl.types.insert(ARCHIVE_URL.into(), "text/html".into());
        let s = fetch(&m, &cache, &mut dl, &[], &mut quiet);
        assert_eq!(s.failed.len(), 2);
        let why = &s.failed[0].reason;
        assert!(
            why.contains("opus/rfc8251") && why.contains("text/html"),
            "{why}"
        );
        assert!(!cache.archive_path(&m.archives()[0]).exists());
        assert!(!cache.temp_archive_path(&m.archives()[0]).exists());
        assert!(cache.verify(&m.entries()[0]).is_err());
    }

    #[test]
    fn a_member_missing_from_the_archive_fails_alone_and_the_archive_is_kept() {
        let tgz = tar_gz(&[("v/a.bit", b"aaa")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa"), ("v/gone.dec", b"x")]);
        let cache = Cache::new(temp_dir("arch-missing"));
        let s = fetch(&m, &cache, &mut net_archive(&tgz), &[], &mut quiet);
        assert_eq!((s.fetched, s.failed.len()), (1, 1));
        assert!(
            s.failed[0].reason.contains("not in the archive"),
            "{:?}",
            s.failed
        );
        assert!(cache.archive_path(&m.archives()[0]).is_file());
    }

    #[test]
    fn a_member_with_the_wrong_content_is_refused() {
        let tgz = tar_gz(&[("v/a.bit", b"aaa")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aab")]);
        let cache = Cache::new(temp_dir("arch-content"));
        let s = fetch(&m, &cache, &mut net_archive(&tgz), &[], &mut quiet);
        assert!(s.failed[0].reason.contains("sha256"), "{:?}", s.failed);
        let e = &m.entries()[0];
        assert!(!cache.path(e).exists() && !cache.temp_path(e).exists());
    }

    #[test]
    fn a_member_larger_than_the_manifest_says_is_not_read() {
        let tgz = tar_gz(&[("v/a.bit", &[7u8; 5000][..])]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"short")]);
        let cache = Cache::new(temp_dir("arch-big"));
        let s = fetch(&m, &cache, &mut net_archive(&tgz), &[], &mut quiet);
        assert!(s.failed[0].reason.contains("size"), "{:?}", s.failed);
        assert!(!cache.path(&m.entries()[0]).exists());
    }

    #[test]
    fn members_match_by_full_path_not_by_file_name() {
        let tgz = tar_gz(&[("other/a.bit", b"WRONG"), ("v/a.bit", b"aaa")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa")]);
        let cache = Cache::new(temp_dir("arch-path"));
        let s = fetch(&m, &cache, &mut net_archive(&tgz), &[], &mut quiet);
        assert!(s.is_ok(), "{s:?}");
        assert_eq!(std::fs::read(cache.path(&m.entries()[0])).unwrap(), b"aaa");
    }

    #[test]
    fn hostile_paths_refuse_the_archive_and_write_nothing_outside_the_cache() {
        use tar::EntryType::Regular;
        for evil in ["../evil.bit", "v/../../evil.bit", "/tmp/evil.bit"] {
            let tgz = tar_gz_items(&[(evil, Regular, b"evil"), ("v/a.bit", Regular, b"aaa")]);
            let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa")]);
            let root = temp_dir("arch-evil");
            let cache = Cache::new(root.join("cache"));
            let s = fetch(&m, &cache, &mut net_archive(&tgz), &[], &mut quiet);
            assert_eq!(s.failed.len(), 1, "{evil}");
            assert!(
                s.failed[0].reason.contains("unsafe"),
                "{evil}: {:?}",
                s.failed
            );
            assert!(!root.join("evil.bit").exists());
            assert!(!cache.path(&m.entries()[0]).exists());
        }
    }

    #[test]
    fn links_and_devices_are_refused() {
        use tar::EntryType::{Block, Char, Fifo, Link, Regular, Symlink};
        for ty in [Symlink, Link, Char, Block, Fifo] {
            let tgz = tar_gz_items(&[("v/x", ty, b""), ("v/a.bit", Regular, b"aaa")]);
            let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa")]);
            let cache = Cache::new(temp_dir("arch-link"));
            let s = fetch(&m, &cache, &mut net_archive(&tgz), &[], &mut quiet);
            assert_eq!(s.failed.len(), 1, "{ty:?}");
            assert!(
                s.failed[0].reason.contains("link or device"),
                "{ty:?}: {:?}",
                s.failed
            );
            assert!(!cache.path(&m.entries()[0]).exists());
        }
    }

    #[test]
    fn a_wanted_member_that_is_a_link_is_refused() {
        use tar::EntryType::Symlink;
        let tgz = tar_gz_items(&[("v/a.bit", Symlink, b"")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa")]);
        let cache = Cache::new(temp_dir("arch-wanted-link"));
        let s = fetch(&m, &cache, &mut net_archive(&tgz), &[], &mut quiet);
        assert_eq!(s.failed.len(), 1);
        assert!(!cache.path(&m.entries()[0]).exists());
    }

    #[test]
    fn a_corrupt_gzip_stream_fails_cleanly() {
        let good = tar_gz(&[("v/a.bit", &[1u8; 4000][..])]);
        let mut bad = good.clone();
        let mid = bad.len() / 2;
        bad.truncate(mid);
        // Size and hash match the truncated bytes, so only reading it can fail.
        let m = archive_manifest(&bad, &[("v/a.bit", &[1u8; 4000][..])]);
        let mut dl = net_archive(&bad);
        let cache = Cache::new(temp_dir("arch-trunc"));
        let s = fetch(&m, &cache, &mut dl, &[], &mut quiet);
        assert_eq!(s.failed.len(), 1);
        assert!(!cache.path(&m.entries()[0]).exists());
    }

    #[test]
    fn seed_copies_by_name_within_the_matching_directory() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let b = entry("flac/faulty/01", "a.flac", b"fff");
        let c = entry("flac/subset/02", "c.flac", b"ccc");
        let src = temp_dir("seed-src");
        for (d, data) in [("subset", &b"aaa"[..]), ("faulty", &b"fff"[..])] {
            std::fs::create_dir_all(src.join("suite").join(d)).unwrap();
            std::fs::write(src.join("suite").join(d).join("a.flac"), data).unwrap();
        }
        let cache = Cache::new(temp_dir("seed-dst"));
        let s = seed(&manifest(&[&a, &b, &c]), &cache, &src, &[]);
        assert_eq!((s.fetched, s.failed.len()), (2, 1));
        assert_eq!(s.failed[0].id, "flac/subset/02");
        assert_eq!(cache.verify(&a), Ok(()));
        assert_eq!(cache.verify(&b), Ok(()));
    }

    #[test]
    fn seed_finds_a_file_in_any_directory_when_the_name_is_unique() {
        let a = entry("vorbis/xiph/a.ogg", "a.ogg", b"ogg");
        let src = temp_dir("seed-any");
        std::fs::create_dir_all(src.join("vorbis")).unwrap();
        std::fs::write(src.join("vorbis/a.ogg"), b"ogg").unwrap();
        let cache = Cache::new(temp_dir("seed-any-dst"));
        let s = seed(&manifest(&[&a]), &cache, &src, &[]);
        assert_eq!(s.fetched, 1, "{s:?}");
        assert_eq!(cache.verify(&a), Ok(()));
    }

    #[test]
    fn seed_rejects_a_mismatching_file_and_skips_valid_ones() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let src = temp_dir("seed-bad-src");
        std::fs::create_dir_all(src.join("subset")).unwrap();
        std::fs::write(src.join("subset/a.flac"), b"zzz").unwrap();
        let cache = Cache::new(temp_dir("seed-bad-dst"));
        let m = manifest(&[&a]);
        let s = seed(&m, &cache, &src, &[]);
        assert_eq!(s.failed.len(), 1);
        assert!(!cache.path(&a).exists() && !cache.temp_path(&a).exists());
        std::fs::write(src.join("subset/a.flac"), b"aaa").unwrap();
        assert_eq!(seed(&m, &cache, &src, &[]).fetched, 1);
        assert_eq!(seed(&m, &cache, &src, &[]).skipped, 1);
    }

    #[test]
    fn seed_ignores_a_missing_source_when_cache_is_already_valid() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let cache = Cache::new(temp_dir("seed-have"));
        let mut dl = fake(&[(&a, Ok(b"aaa"))]);
        let m = manifest(&[&a]);
        fetch(&m, &cache, &mut dl, &[], &mut quiet);
        let s = seed(&m, &cache, Path::new("/nonexistent-dir"), &[]);
        assert!(s.is_ok() && s.skipped == 1);
    }

    #[test]
    fn seed_honours_only() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let b = entry("opus/x/b.bit", "b.bit", b"bbb");
        let src = temp_dir("seed-only");
        std::fs::write(src.join("a.flac"), b"aaa").unwrap();
        std::fs::write(src.join("b.bit"), b"bbb").unwrap();
        let cache = Cache::new(temp_dir("seed-only-dst"));
        let s = seed(&manifest(&[&a, &b]), &cache, &src, &["opus/".to_string()]);
        assert_eq!(s.fetched, 1);
        assert!(cache.verify(&a).is_err());
    }

    #[test]
    fn seed_extracts_members_from_an_archive_in_the_source_directory() {
        let tgz = tar_gz(&[("v/a.bit", b"aaa"), ("v/b.dec", b"bbbb")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa"), ("v/b.dec", b"bbbb")]);
        let src = temp_dir("seed-arch");
        std::fs::create_dir_all(src.join("opus")).unwrap();
        std::fs::write(src.join("opus/v.tar.gz"), &tgz).unwrap();
        let cache = Cache::new(temp_dir("seed-arch-dst"));
        let s = seed(&m, &cache, &src, &[]);
        assert_eq!((s.fetched, s.failed.len()), (2, 0), "{s:?}");
        for e in m.entries() {
            assert_eq!(cache.verify(e), Ok(()));
        }
        // Seeding never leaves a copy of the archive behind.
        assert!(!cache.archive_path(&m.archives()[0]).exists());
    }

    #[test]
    fn seed_accepts_the_archive_file_itself() {
        let tgz = tar_gz(&[("v/a.bit", b"aaa")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa")]);
        let src = temp_dir("seed-arch-file");
        let file = src.join("whatever.tgz");
        std::fs::write(&file, &tgz).unwrap();
        let cache = Cache::new(temp_dir("seed-arch-file-dst"));
        let s = seed(&m, &cache, &file, &[]);
        assert_eq!(s.fetched, 1, "{s:?}");
    }

    #[test]
    fn seed_takes_already_extracted_members_and_checks_a_wrong_archive() {
        let tgz = tar_gz(&[("v/a.bit", b"aaa"), ("v/b.dec", b"bbbb")]);
        let m = archive_manifest(&tgz, &[("v/a.bit", b"aaa"), ("v/b.dec", b"bbbb")]);
        let src = temp_dir("seed-extracted");
        std::fs::create_dir_all(src.join("v")).unwrap();
        std::fs::write(src.join("v/a.bit"), b"aaa").unwrap();
        std::fs::write(src.join("v/b.dec"), b"bbbb").unwrap();
        let cache = Cache::new(temp_dir("seed-extracted-dst"));
        assert_eq!(seed(&m, &cache, &src, &[]).fetched, 2);
        // A file with the archive's name but other content is not trusted.
        let src = temp_dir("seed-wrong-arch");
        std::fs::write(src.join("v.tar.gz"), b"nope").unwrap();
        let cache = Cache::new(temp_dir("seed-wrong-arch-dst"));
        let s = seed(&m, &cache, &src, &[]);
        assert_eq!(s.failed.len(), 2);
    }

    #[test]
    fn curl_downloader_copies_a_file_url_and_reports_failure() {
        let dir = temp_dir("curl");
        let src = dir.join("src.bin");
        std::fs::write(&src, b"payload").unwrap();
        let dest = dir.join("out.bin");
        let mut dl = CurlDownloader;
        dl.download(&format!("file://{}", src.display()), &dest, 1000)
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
        let err = dl
            .download(
                &format!("file://{}", dir.join("nope").display()),
                &dest,
                1000,
            )
            .unwrap_err();
        assert!(err.contains("curl"), "{err}");
    }
}
