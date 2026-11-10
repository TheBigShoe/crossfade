//! The `corpus.toml` manifest: which files exist, where they come from and
//! what a decoder is held to for each.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// What the conformance tier holds a decoder to for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Decodes to the end and matches the stored MD5.
    Decode,
    /// Decodes correctly or returns an error; never wrong audio or a panic.
    DecodeOrReject,
    /// Must not panic or hang; any result is acceptable.
    NoPanic,
}

impl Expect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decode => "decode",
            Self::DecodeOrReject => "decode-or-reject",
            Self::NoPanic => "no-panic",
        }
    }
}

/// One corpus file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Stable short id, `suite/dir/number`; doubles as the cache sub-path.
    pub id: String,
    /// The file's real name.
    pub name: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
    pub licence: String,
    pub expect: Expect,
    /// STREAMINFO MD5 as hex, when the file stores one.
    pub md5: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug)]
pub enum ManifestError {
    Io { path: PathBuf, source: std::io::Error },
    Syntax(String),
    Invalid { id: String, reason: String },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Syntax(e) => write!(f, "manifest syntax error: {e}"),
            Self::Invalid { id, reason } => write!(f, "manifest entry {id}: {reason}"),
        }
    }
}

impl std::error::Error for ManifestError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    entries: Vec<Entry>,
}

// Optional fields everywhere so a missing key is reported against its entry
// rather than as an anonymous serde error.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    id: Option<String>,
    name: Option<String>,
    url: Option<String>,
    bytes: Option<u64>,
    sha256: Option<String>,
    licence: Option<String>,
    expect: Option<String>,
    md5: Option<String>,
    note: Option<String>,
}

#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    file: Vec<RawEntry>,
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn convert(raw: RawEntry, index: usize) -> Result<Entry, ManifestError> {
    let id = raw.id.clone().filter(|s| !s.is_empty());
    let label = id.clone().unwrap_or_else(|| format!("#{index}"));
    let bad = |reason: String| ManifestError::Invalid { id: label.clone(), reason };
    let id = id.ok_or_else(|| bad("missing or empty key `id`".into()))?;
    let need = |v: Option<String>, key: &str| {
        v.filter(|s| !s.is_empty())
            .ok_or_else(|| bad(format!("missing or empty key `{key}`")))
    };
    let name = need(raw.name, "name")?;
    let url = need(raw.url, "url")?;
    let sha256 = need(raw.sha256, "sha256")?;
    let licence = need(raw.licence, "licence")?;
    let expect = need(raw.expect, "expect")?;
    let bytes = raw.bytes.ok_or_else(|| bad("missing key `bytes`".into()))?;
    if !url.starts_with("https://") {
        return Err(bad(format!("url must be https: {url}")));
    }
    if !is_hex(&sha256, 64) {
        return Err(bad("sha256 must be 64 hex digits".into()));
    }
    if raw.md5.as_deref().is_some_and(|m| !is_hex(m, 32)) {
        return Err(bad("md5 must be 32 hex digits".into()));
    }
    let expect = match expect.as_str() {
        "decode" => Expect::Decode,
        "decode-or-reject" => Expect::DecodeOrReject,
        "no-panic" => Expect::NoPanic,
        other => return Err(bad(format!("unknown expect value `{other}`"))),
    };
    Ok(Entry {
        id,
        name,
        url,
        bytes,
        sha256: sha256.to_ascii_lowercase(),
        licence,
        expect,
        md5: raw.md5.map(|m| m.to_ascii_lowercase()),
        note: raw.note,
    })
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let text = std::fs::read_to_string(path)
            .map_err(|source| ManifestError::Io { path: path.to_path_buf(), source })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest =
            toml::from_str(text).map_err(|e| ManifestError::Syntax(e.to_string()))?;
        let mut seen = HashSet::new();
        let mut entries = Vec::with_capacity(raw.file.len());
        for (i, r) in raw.file.into_iter().enumerate() {
            let e = convert(r, i)?;
            if !seen.insert(e.id.clone()) {
                return Err(ManifestError::Invalid {
                    id: e.id,
                    reason: "duplicate id".into(),
                });
            }
            entries.push(e);
        }
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn entry(extra: &str) -> String {
        format!(
            "[[file]]\nid = \"flac/subset/01\"\nname = \"01 - a.flac\"\n\
             url = \"https://example.org/a.flac\"\nbytes = 10\nsha256 = \"{HASH}\"\n\
             licence = \"CC0-1.0\"\nexpect = \"decode\"\n{extra}"
        )
    }

    fn err(text: &str) -> String {
        Manifest::parse(text).unwrap_err().to_string()
    }

    #[test]
    fn parses_a_full_entry() {
        let m = Manifest::parse(&entry(
            "md5 = \"00112233445566778899aabbccddeeff\"\nnote = \"hi\"\n",
        ))
        .unwrap();
        let e = &m.entries()[0];
        assert_eq!(e.id, "flac/subset/01");
        assert_eq!(e.name, "01 - a.flac");
        assert_eq!(e.bytes, 10);
        assert_eq!(e.sha256, HASH);
        assert_eq!(e.expect, Expect::Decode);
        assert_eq!(e.md5.as_deref(), Some("00112233445566778899aabbccddeeff"));
        assert_eq!(e.note.as_deref(), Some("hi"));
        assert!(m.get("flac/subset/01").is_some());
        assert!(m.get("nope").is_none());
    }

    #[test]
    fn optional_keys_may_be_absent() {
        let m = Manifest::parse(&entry("")).unwrap();
        assert!(m.entries()[0].md5.is_none() && m.entries()[0].note.is_none());
    }

    #[test]
    fn expect_values() {
        for (text, want) in [
            ("decode", Expect::Decode),
            ("decode-or-reject", Expect::DecodeOrReject),
            ("no-panic", Expect::NoPanic),
        ] {
            let t = entry("").replace("\"decode\"", &format!("\"{text}\""));
            assert_eq!(Manifest::parse(&t).unwrap().entries()[0].expect, want);
            assert_eq!(want.as_str(), text);
        }
    }

    #[test]
    fn duplicate_ids_name_the_entry() {
        let e = err(&format!("{}{}", entry(""), entry("")));
        assert!(e.contains("flac/subset/01") && e.contains("duplicate"), "{e}");
    }

    #[test]
    fn bad_sha256_names_the_entry() {
        let e = err(&entry("").replace(HASH, "abc"));
        assert!(e.contains("flac/subset/01") && e.contains("sha256"), "{e}");
        let e = err(&entry("").replace(HASH, &"g".repeat(64)));
        assert!(e.contains("sha256"), "{e}");
    }

    #[test]
    fn non_https_url_is_rejected() {
        let e = err(&entry("").replace("https://", "http://"));
        assert!(e.contains("flac/subset/01") && e.contains("https"), "{e}");
    }

    #[test]
    fn unknown_expect_is_rejected() {
        let e = err(&entry("").replace("\"decode\"", "\"maybe\""));
        assert!(e.contains("flac/subset/01") && e.contains("maybe"), "{e}");
    }

    #[test]
    fn bad_md5_is_rejected() {
        let e = err(&entry("md5 = \"xyz\"\n"));
        assert!(e.contains("md5"), "{e}");
    }

    #[test]
    fn missing_key_and_syntax_errors() {
        let e = err(&entry("").replace("bytes = 10\n", ""));
        assert!(e.contains("flac/subset/01") && e.contains("bytes"), "{e}");
        assert!(err("[[file]\n").contains("manifest"));
    }

    #[test]
    fn empty_id_or_name_is_rejected() {
        assert!(err(&entry("").replace("flac/subset/01", "")).contains("id"));
        assert!(err(&entry("").replace("01 - a.flac", "")).contains("name"));
    }

    #[test]
    fn load_reports_a_missing_file() {
        let e = Manifest::load(std::path::Path::new("/nonexistent/corpus.toml"));
        assert!(e.unwrap_err().to_string().contains("/nonexistent/corpus.toml"));
    }

    #[test]
    fn load_reads_a_file() {
        let dir = std::env::temp_dir().join("crossfade-corpus-manifest-load");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("corpus.toml");
        std::fs::write(&p, entry("")).unwrap();
        assert_eq!(Manifest::load(&p).unwrap().entries().len(), 1);
    }
}
