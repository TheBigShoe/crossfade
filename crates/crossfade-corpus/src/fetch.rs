//! Getting files into the cache: download or copy to a temp name, verify,
//! then rename, so the final name only ever holds a verified file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cache::verify_file;
use crate::{Cache, Entry, Manifest};

/// Seam for the network, so tests inject a fake.
pub trait Downloader {
    /// Write the body of `url` to `dest`.
    fn download(&mut self, url: &str, dest: &Path) -> Result<(), String>;
}

/// Shells out to `curl`, which keeps a TLS stack out of the dependency tree.
#[derive(Debug, Default)]
pub struct CurlDownloader;

impl Downloader for CurlDownloader {
    fn download(&mut self, url: &str, dest: &Path) -> Result<(), String> {
        let out = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                "--output",
            ])
            .arg(dest)
            .arg(url)
            .output()
            .map_err(|e| format!("cannot run curl: {e}"))?;
        if out.status.success() {
            Ok(())
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

fn install(
    cache: &Cache,
    entry: &Entry,
    produce: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let tmp = cache.temp_path(entry);
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let result = produce(&tmp)
        .and_then(|()| verify_file(entry, &tmp).map_err(|p| p.to_string()))
        .and_then(|()| std::fs::rename(&tmp, cache.path(entry)).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Download every entry whose id starts with `only` (all when `None`) that is
/// not already valid. One bad file never stops the rest.
pub fn fetch(
    manifest: &Manifest,
    cache: &Cache,
    downloader: &mut dyn Downloader,
    only: Option<&str>,
    progress: &mut dyn FnMut(Event<'_>),
) -> Summary {
    let mut summary = Summary::default();
    for entry in manifest.entries() {
        if only.is_some_and(|p| !entry.id.starts_with(p)) {
            continue;
        }
        let outcome = if cache.verify(entry).is_ok() {
            Outcome::Skipped
        } else {
            progress(Event::Starting(entry));
            match install(cache, entry, |tmp| downloader.download(&entry.url, tmp)) {
                Ok(()) => Outcome::Fetched,
                Err(e) => Outcome::Failed(e),
            }
        };
        progress(Event::Finished(entry, &outcome));
        summary.record(entry, &outcome);
    }
    summary
}

fn index_tree(dir: &Path, out: &mut HashMap<(String, String), PathBuf>) {
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
            index_tree(&path, out);
        } else if let Some(name) = path.file_name() {
            out.entry((parent.clone(), name.to_string_lossy().into_owned()))
                .or_insert(path);
        }
    }
}

/// Copy files found under `from` into the cache, matching on file name inside
/// the sub-directory named by the entry's id (`subset`, `uncommon`, ...), and
/// verify each. For offline mirrors; entries not found there are failures
/// unless the cache already holds them.
pub fn seed(manifest: &Manifest, cache: &Cache, from: &Path) -> Summary {
    let mut index = HashMap::new();
    index_tree(from, &mut index);
    let mut summary = Summary::default();
    for entry in manifest.entries() {
        let outcome = if cache.verify(entry).is_ok() {
            Outcome::Skipped
        } else {
            let group = entry.id.rsplit('/').nth(1).unwrap_or_default().to_string();
            match index.get(&(group, entry.name.clone())) {
                None => Outcome::Failed(format!("not found under {}", from.display())),
                Some(src) => match install(cache, entry, |tmp| {
                    std::fs::copy(src, tmp)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                }) {
                    Ok(()) => Outcome::Fetched,
                    Err(e) => Outcome::Failed(e),
                },
            }
        };
        summary.record(entry, &outcome);
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{entry, temp_dir};
    use crate::{Cache, Manifest};
    use std::collections::HashMap;

    struct Fake {
        bodies: HashMap<String, Result<Vec<u8>, String>>,
        calls: Vec<String>,
    }

    impl Downloader for Fake {
        fn download(&mut self, url: &str, dest: &Path) -> Result<(), String> {
            self.calls.push(url.to_string());
            match self.bodies.get(url) {
                Some(Ok(b)) => std::fs::write(dest, b).map_err(|e| e.to_string()),
                Some(Err(e)) => Err(e.clone()),
                None => Err("404".into()),
            }
        }
    }

    fn manifest(entries: &[&crate::Entry]) -> Manifest {
        let mut text = String::new();
        for e in entries {
            text += &format!(
                "[[file]]\nid = \"{}\"\nname = \"{}\"\nurl = \"{}\"\nbytes = {}\nsha256 = \"{}\"\n\
                 licence = \"CC0-1.0\"\nexpect = \"decode\"\n",
                e.id, e.name, e.url, e.bytes, e.sha256
            );
        }
        Manifest::parse(&text).unwrap()
    }

    fn fake(items: &[(&crate::Entry, Result<&[u8], &str>)]) -> Fake {
        Fake {
            bodies: items
                .iter()
                .map(|(e, r)| (e.url.clone(), r.map(<[u8]>::to_vec).map_err(str::to_string)))
                .collect(),
            calls: vec![],
        }
    }

    fn quiet(_: Event<'_>) {}

    #[test]
    fn fetches_and_verifies() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let cache = Cache::new(temp_dir("fetch-ok"));
        let mut dl = fake(&[(&a, Ok(b"aaa"))]);
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, None, &mut quiet);
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
        fetch(&m, &cache, &mut dl, None, &mut quiet);
        let s = fetch(&m, &cache, &mut dl, None, &mut quiet);
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
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, None, &mut quiet);
        assert_eq!(s.fetched, 1);
        assert_eq!(cache.verify(&a), Ok(()));
    }

    #[test]
    fn wrong_hash_leaves_nothing_behind_and_is_reported() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let cache = Cache::new(temp_dir("fetch-hash"));
        let mut dl = fake(&[(&a, Ok(b"bbb"))]);
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, None, &mut quiet);
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
        let s = fetch(&manifest(&[&a]), &cache, &mut dl, None, &mut quiet);
        assert!(s.failed[0].reason.contains("size"));
        assert!(!cache.temp_path(&a).exists());
    }

    #[test]
    fn a_failed_download_does_not_stop_the_others() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let b = entry("flac/subset/02", "b.flac", b"bbb");
        let cache = Cache::new(temp_dir("fetch-fail"));
        let mut dl = fake(&[(&a, Err("boom")), (&b, Ok(b"bbb"))]);
        let s = fetch(&manifest(&[&a, &b]), &cache, &mut dl, None, &mut quiet);
        assert_eq!((s.fetched, s.failed.len()), (1, 1));
        assert!(s.failed[0].reason.contains("boom"));
        assert_eq!(cache.verify(&b), Ok(()));
        assert!(!cache.temp_path(&a).exists());
    }

    #[test]
    fn only_filters_by_id_prefix() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let b = entry("flac/faulty/01", "b.flac", b"bbb");
        let cache = Cache::new(temp_dir("fetch-only"));
        let mut dl = fake(&[(&a, Ok(b"aaa")), (&b, Ok(b"bbb"))]);
        let s = fetch(
            &manifest(&[&a, &b]),
            &cache,
            &mut dl,
            Some("flac/faulty"),
            &mut quiet,
        );
        assert_eq!(s.fetched, 1);
        assert_eq!(dl.calls, vec![b.url.clone()]);
    }

    #[test]
    fn progress_reports_every_file() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let b = entry("flac/subset/02", "b.flac", b"bbb");
        let cache = Cache::new(temp_dir("fetch-progress"));
        let mut dl = fake(&[(&a, Ok(b"aaa")), (&b, Err("x"))]);
        let mut lines = vec![];
        fetch(&manifest(&[&a, &b]), &cache, &mut dl, None, &mut |ev| {
            if let Event::Finished(e, o) = ev {
                lines.push(format!("{} {o:?}", e.id));
            }
        });
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Fetched") && lines[1].contains("Failed"));
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
        let s = seed(&manifest(&[&a, &b, &c]), &cache, &src);
        assert_eq!((s.fetched, s.failed.len()), (2, 1));
        assert_eq!(s.failed[0].id, "flac/subset/02");
        assert_eq!(cache.verify(&a), Ok(()));
        assert_eq!(cache.verify(&b), Ok(()));
    }

    #[test]
    fn seed_rejects_a_mismatching_file_and_skips_valid_ones() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let src = temp_dir("seed-bad-src");
        std::fs::create_dir_all(src.join("subset")).unwrap();
        std::fs::write(src.join("subset/a.flac"), b"zzz").unwrap();
        let cache = Cache::new(temp_dir("seed-bad-dst"));
        let m = manifest(&[&a]);
        let s = seed(&m, &cache, &src);
        assert_eq!(s.failed.len(), 1);
        assert!(!cache.path(&a).exists() && !cache.temp_path(&a).exists());
        std::fs::write(src.join("subset/a.flac"), b"aaa").unwrap();
        assert_eq!(seed(&m, &cache, &src).fetched, 1);
        assert_eq!(seed(&m, &cache, &src).skipped, 1);
    }

    #[test]
    fn seed_ignores_a_missing_source_when_cache_is_already_valid() {
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let cache = Cache::new(temp_dir("seed-have"));
        let mut dl = fake(&[(&a, Ok(b"aaa"))]);
        let m = manifest(&[&a]);
        fetch(&m, &cache, &mut dl, None, &mut quiet);
        let s = seed(&m, &cache, Path::new("/nonexistent-dir"));
        assert!(s.is_ok() && s.skipped == 1);
    }

    #[test]
    fn curl_downloader_copies_a_file_url_and_reports_failure() {
        let dir = temp_dir("curl");
        let src = dir.join("src.bin");
        std::fs::write(&src, b"payload").unwrap();
        let dest = dir.join("out.bin");
        let mut dl = CurlDownloader;
        dl.download(&format!("file://{}", src.display()), &dest)
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
        let err = dl
            .download(&format!("file://{}", dir.join("nope").display()), &dest)
            .unwrap_err();
        assert!(err.contains("curl"), "{err}");
    }
}
