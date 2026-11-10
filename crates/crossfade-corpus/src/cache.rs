#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{entry, temp_dir};

    fn put(cache: &Cache, e: &Entry, data: &[u8]) {
        let p = cache.path(e);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
    }

    #[test]
    fn path_layout_is_dir_id_name() {
        let cache = Cache::new(PathBuf::from("/c"));
        let e = entry("flac/subset/01", "01 - a.flac", b"x");
        assert_eq!(cache.path(&e), PathBuf::from("/c/flac/subset/01/01 - a.flac"));
        assert_eq!(cache.dir(), Path::new("/c"));
    }

    #[test]
    fn verify_accepts_a_good_file() {
        let cache = Cache::new(temp_dir("verify-ok"));
        let e = entry("flac/subset/01", "a.flac", b"hello");
        put(&cache, &e, b"hello");
        assert_eq!(cache.verify(&e), Ok(()));
    }

    #[test]
    fn verify_reports_missing() {
        let cache = Cache::new(temp_dir("verify-missing"));
        let e = entry("flac/subset/01", "a.flac", b"hello");
        assert_eq!(cache.verify(&e), Err(Problem::Missing));
    }

    #[test]
    fn verify_reports_wrong_size() {
        let cache = Cache::new(temp_dir("verify-size"));
        let e = entry("flac/subset/01", "a.flac", b"hello");
        put(&cache, &e, b"hello!");
        assert_eq!(
            cache.verify(&e),
            Err(Problem::WrongSize { expected: 5, actual: 6 })
        );
    }

    #[test]
    fn verify_reports_wrong_hash() {
        let cache = Cache::new(temp_dir("verify-hash"));
        let e = entry("flac/subset/01", "a.flac", b"hello");
        put(&cache, &e, b"jello");
        match cache.verify(&e) {
            Err(Problem::WrongHash { expected, actual }) => {
                assert_eq!(expected, e.sha256);
                assert_ne!(actual, expected);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_temp_file_is_never_mistaken_for_the_cached_file() {
        let cache = Cache::new(temp_dir("verify-part"));
        let e = entry("flac/subset/01", "a.flac", b"hello");
        let part = cache.temp_path(&e);
        std::fs::create_dir_all(part.parent().unwrap()).unwrap();
        std::fs::write(&part, b"hello").unwrap();
        assert_ne!(part, cache.path(&e));
        assert_eq!(cache.verify(&e), Err(Problem::Missing));
    }

    #[test]
    fn problems_read_clearly() {
        assert!(Problem::Missing.to_string().contains("missing"));
        let s = Problem::WrongSize { expected: 1, actual: 2 }.to_string();
        assert!(s.contains('1') && s.contains('2'));
        let h = Problem::WrongHash { expected: "aa".into(), actual: "bb".into() }.to_string();
        assert!(h.contains("aa") && h.contains("bb"));
    }

    #[test]
    fn default_dir_prefers_the_environment() {
        let root = Path::new("/ws");
        assert_eq!(
            Cache::dir_from(Some("/elsewhere".into()), root),
            PathBuf::from("/elsewhere")
        );
        assert_eq!(Cache::dir_from(None, root), PathBuf::from("/ws/.corpus-cache"));
        assert_eq!(Cache::dir_from(Some("".into()), root), PathBuf::from("/ws/.corpus-cache"));
    }
}
