//! What conformance tests call: a verified path, or a loud failure.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use crate::{Cache, Manifest, Problem};

#[derive(Debug)]
pub enum RequireError {
    UnknownId(String),
    Unusable {
        id: String,
        path: PathBuf,
        problem: Problem,
    },
}

impl fmt::Display for RequireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownId(id) => write!(f, "corpus id `{id}` is not listed in corpus.toml"),
            Self::Unusable { id, path, problem } => write!(
                f,
                "corpus file `{id}` is unusable ({problem}) at {}; run `just corpus` to fetch the \
                 conformance files (set CROSSFADE_CORPUS_DIR to share a cache)",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RequireError {}

/// The workspace this crate was built in.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// Some files are tens of megabytes; hash each at most once per process.
static VERIFIED: LazyLock<Mutex<HashSet<(PathBuf, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn try_require(manifest: &Manifest, cache: &Cache, id: &str) -> Result<PathBuf, RequireError> {
    let entry = manifest
        .get(id)
        .ok_or_else(|| RequireError::UnknownId(id.to_string()))?;
    let path = cache.path(entry);
    let key = (path.clone(), entry.sha256.clone());
    // A poisoned lock only means another test panicked; the set is still valid.
    let mut done = VERIFIED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !done.contains(&key) {
        cache
            .verify(entry)
            .map_err(|problem| RequireError::Unusable {
                id: id.to_string(),
                path: path.clone(),
                problem,
            })?;
        done.insert(key);
    }
    Ok(path)
}

/// Like [`try_require`] but panics: a missing corpus must fail the
/// conformance tier, never skip it.
pub fn require_from(manifest: &Manifest, cache: &Cache, id: &str) -> PathBuf {
    try_require(manifest, cache, id).unwrap_or_else(|e| panic!("{e}"))
}

/// The verified cached path of corpus file `id`, using the workspace's
/// `corpus.toml` and the default cache. Panics when it is absent or invalid.
pub fn require(id: &str) -> PathBuf {
    let root = workspace_root();
    let manifest = Manifest::load(&root.join("corpus.toml"))
        .unwrap_or_else(|e| panic!("{e}; run `just corpus` from a complete checkout"));
    let dir = Cache::dir_from_env(&root);
    require_from(&manifest, &Cache::new(dir), id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Manifest;
    use crate::testutil::{entry, temp_dir};

    fn setup(tag: &str, data: &[u8]) -> (Manifest, Cache, crate::Entry) {
        let e = entry("flac/subset/01", "a.flac", data);
        let m = Manifest::parse(&format!(
            "[[file]]\nid = \"{}\"\nname = \"{}\"\nurl = \"{}\"\nbytes = {}\nsha256 = \"{}\"\n\
             licence = \"CC0-1.0\"\nexpect = \"decode\"\n",
            e.id,
            e.name,
            e.url().unwrap(),
            e.bytes,
            e.sha256
        ))
        .unwrap();
        (m, Cache::new(temp_dir(tag)), e)
    }

    fn place(cache: &Cache, e: &crate::Entry, data: &[u8]) {
        std::fs::create_dir_all(cache.path(e).parent().unwrap()).unwrap();
        std::fs::write(cache.path(e), data).unwrap();
    }

    #[test]
    fn returns_the_path_of_a_valid_file() {
        let (m, c, e) = setup("req-ok", b"abc");
        place(&c, &e, b"abc");
        assert_eq!(require_from(&m, &c, "flac/subset/01"), c.path(&e));
    }

    #[test]
    #[should_panic(expected = "just corpus")]
    fn panics_telling_the_reader_to_fetch_when_absent() {
        let (m, c, _) = setup("req-absent", b"abc");
        require_from(&m, &c, "flac/subset/01");
    }

    #[test]
    fn invalid_file_is_an_error_naming_the_problem() {
        let (m, c, e) = setup("req-bad", b"abc");
        place(&c, &e, b"xyz");
        let msg = try_require(&m, &c, "flac/subset/01")
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("just corpus") && msg.contains("sha256"),
            "{msg}"
        );
    }

    #[test]
    fn unknown_id_is_an_error() {
        let (m, c, _) = setup("req-unknown", b"abc");
        let msg = try_require(&m, &c, "flac/nope/99").unwrap_err().to_string();
        assert!(
            msg.contains("flac/nope/99") && msg.contains("corpus.toml"),
            "{msg}"
        );
    }

    #[test]
    fn hashing_happens_once_per_process() {
        let (m, c, e) = setup("req-memo", b"abc");
        place(&c, &e, b"abc");
        assert!(try_require(&m, &c, "flac/subset/01").is_ok());
        // Same size, different content: only a memo can still pass.
        place(&c, &e, b"xyz");
        assert!(try_require(&m, &c, "flac/subset/01").is_ok());
    }

    #[test]
    fn workspace_root_holds_the_committed_manifest() {
        assert!(workspace_root().join("corpus.toml").is_file());
    }
}
