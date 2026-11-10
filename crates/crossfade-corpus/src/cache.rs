//! The on-disk cache: `<dir>/<id>/<name>`, verified by size then SHA-256.

use std::ffi::OsString;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::Entry;

/// Why a cached file cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Missing,
    WrongSize { expected: u64, actual: u64 },
    WrongHash { expected: String, actual: String },
    Unreadable(String),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(f, "missing"),
            Self::WrongSize { expected, actual } => {
                write!(f, "wrong size: expected {expected} bytes, found {actual}")
            }
            Self::WrongHash { expected, actual } => {
                write!(f, "wrong sha256: expected {expected}, found {actual}")
            }
            Self::Unreadable(e) => write!(f, "unreadable: {e}"),
        }
    }
}

impl std::error::Error for Problem {}

pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `CROSSFADE_CORPUS_DIR` when set and non-empty, else `<root>/.corpus-cache`.
    pub(crate) fn dir_from(env: Option<OsString>, root: &Path) -> PathBuf {
        match env {
            Some(d) if !d.is_empty() => PathBuf::from(d),
            _ => root.join(".corpus-cache"),
        }
    }

    pub fn path(&self, entry: &Entry) -> PathBuf {
        self.dir.join(&entry.id).join(&entry.name)
    }

    /// Where a download is staged. Never the final name, so a half-written
    /// file cannot pass for a cached one.
    pub(crate) fn temp_path(&self, entry: &Entry) -> PathBuf {
        self.dir.join(&entry.id).join(format!("{}.part", entry.name))
    }

    pub fn verify(&self, entry: &Entry) -> Result<(), Problem> {
        verify_file(entry, &self.path(entry))
    }
}

pub(crate) fn verify_file(entry: &Entry, path: &Path) -> Result<(), Problem> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Problem::Missing),
        Err(e) => return Err(Problem::Unreadable(e.to_string())),
    };
    if meta.len() != entry.bytes {
        return Err(Problem::WrongSize { expected: entry.bytes, actual: meta.len() });
    }
    let actual = sha256_file(path).map_err(|e| Problem::Unreadable(e.to_string()))?;
    if actual != entry.sha256 {
        return Err(Problem::WrongHash { expected: entry.sha256.clone(), actual });
    }
    Ok(())
}

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
