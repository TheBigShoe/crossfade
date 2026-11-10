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
                .map(|(e, r)| {
                    (e.url.clone(), r.map(<[u8]>::to_vec).map_err(str::to_string))
                })
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
        assert!(s.failed[0].reason.contains("sha256"), "{}", s.failed[0].reason);
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
        let s = fetch(&manifest(&[&a, &b]), &cache, &mut dl, Some("flac/faulty"), &mut quiet);
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
}
