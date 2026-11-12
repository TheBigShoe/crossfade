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
    /// Fetched and verified, never decoded by itself: a reference output, or a
    /// stream whose test lives in a codec's own conformance test.
    Data,
}

impl Expect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decode => "decode",
            Self::DecodeOrReject => "decode-or-reject",
            Self::NoPanic => "no-panic",
            Self::Data => "data",
        }
    }
}

/// Where a file's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Downloaded on its own.
    Url(String),
    /// One member of an `[[archive]]`; `member` is the manifest's word for the
    /// path inside it, never taken from the archive.
    Archive { archive: String, member: String },
}

/// One corpus file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Stable short id, `codec/suite/name`; doubles as the cache sub-path.
    pub id: String,
    /// The file's real name.
    pub name: String,
    pub source: Source,
    pub bytes: u64,
    pub sha256: String,
    pub licence: String,
    pub expect: Expect,
    /// STREAMINFO MD5 as hex, when the file stores one.
    pub md5: Option<String>,
    pub note: Option<String>,
}

impl Entry {
    /// The download URL, for a file fetched on its own.
    pub fn url(&self) -> Option<&str> {
        match &self.source {
            Source::Url(u) => Some(u),
            Source::Archive { .. } => None,
        }
    }
}

/// How an archive is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    TarGz,
}

/// A download that holds several corpus files. Only the members the manifest
/// names are ever extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    pub id: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
    pub licence: String,
    pub format: ArchiveFormat,
}

#[derive(Debug)]
pub enum ManifestError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Syntax(String),
    Invalid {
        id: String,
        reason: String,
    },
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
    archives: Vec<Archive>,
}

// Optional fields everywhere so a missing key is reported against its entry
// rather than as an anonymous serde error.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    id: Option<String>,
    name: Option<String>,
    url: Option<String>,
    archive: Option<String>,
    member: Option<String>,
    bytes: Option<u64>,
    sha256: Option<String>,
    licence: Option<String>,
    expect: Option<String>,
    md5: Option<String>,
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArchive {
    id: Option<String>,
    url: Option<String>,
    bytes: Option<u64>,
    sha256: Option<String>,
    licence: Option<String>,
    format: Option<String>,
}

#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    file: Vec<RawEntry>,
    #[serde(default)]
    archive: Vec<RawArchive>,
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A relative path of plain components: nothing that could climb out of, or
/// start outside, the directory an archive is read for.
pub(crate) fn is_safe_member(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', '\0'])
        && name
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != "..")
}

fn convert(raw: RawEntry, index: usize) -> Result<Entry, ManifestError> {
    let id = raw.id.clone().filter(|s| !s.is_empty());
    let label = id.clone().unwrap_or_else(|| format!("#{index}"));
    let bad = |reason: String| ManifestError::Invalid {
        id: label.clone(),
        reason,
    };
    let id = id.ok_or_else(|| bad("missing or empty key `id`".into()))?;
    let need = |v: Option<String>, key: &str| {
        v.filter(|s| !s.is_empty())
            .ok_or_else(|| bad(format!("missing or empty key `{key}`")))
    };
    let name = need(raw.name, "name")?;
    let sha256 = need(raw.sha256, "sha256")?;
    let licence = need(raw.licence, "licence")?;
    let expect = need(raw.expect, "expect")?;
    let bytes = raw.bytes.ok_or_else(|| bad("missing key `bytes`".into()))?;
    let source = match (raw.url, raw.archive, raw.member) {
        (Some(url), None, None) => {
            if !url.starts_with("https://") {
                return Err(bad(format!("url must be https: {url}")));
            }
            Source::Url(url)
        }
        (None, Some(archive), Some(member)) => {
            if !is_safe_member(&member) {
                return Err(bad(format!(
                    "member `{member}` must be a relative path without `.`, `..` or empty parts"
                )));
            }
            Source::Archive { archive, member }
        }
        (None, Some(_), None) => return Err(bad("an archive file needs a key `member`".into())),
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err(bad(
                "give either `url` or `archive` with `member`, not both".into(),
            ));
        }
        (None, None, Some(_)) => return Err(bad("`member` needs a key `archive`".into())),
        (None, None, None) => {
            return Err(bad("missing key `url` (or `archive` and `member`)".into()));
        }
    };
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
        "data" => Expect::Data,
        other => return Err(bad(format!("unknown expect value `{other}`"))),
    };
    Ok(Entry {
        id,
        name,
        source,
        bytes,
        sha256: sha256.to_ascii_lowercase(),
        licence,
        expect,
        md5: raw.md5.map(|m| m.to_ascii_lowercase()),
        note: raw.note,
    })
}

fn convert_archive(raw: RawArchive, index: usize) -> Result<Archive, ManifestError> {
    let id = raw.id.clone().filter(|s| !s.is_empty());
    let label = format!(
        "archive {}",
        id.clone().unwrap_or_else(|| format!("#{index}"))
    );
    let bad = |reason: String| ManifestError::Invalid {
        id: label.clone(),
        reason,
    };
    let need = |v: Option<String>, key: &str| {
        v.filter(|s| !s.is_empty())
            .ok_or_else(|| bad(format!("missing or empty key `{key}`")))
    };
    let id = id.ok_or_else(|| bad("missing or empty key `id`".into()))?;
    let url = need(raw.url, "url")?;
    let sha256 = need(raw.sha256, "sha256")?;
    let licence = need(raw.licence, "licence")?;
    let format = need(raw.format, "format")?;
    let bytes = raw.bytes.ok_or_else(|| bad("missing key `bytes`".into()))?;
    if !url.starts_with("https://") {
        return Err(bad(format!("url must be https: {url}")));
    }
    if !is_hex(&sha256, 64) {
        return Err(bad("sha256 must be 64 hex digits".into()));
    }
    let format = match format.as_str() {
        "tar.gz" => ArchiveFormat::TarGz,
        other => return Err(bad(format!("unknown archive format `{other}`"))),
    };
    Ok(Archive {
        id,
        url,
        bytes,
        sha256: sha256.to_ascii_lowercase(),
        licence,
        format,
    })
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let text = std::fs::read_to_string(path).map_err(|source| ManifestError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest =
            toml::from_str(text).map_err(|e| ManifestError::Syntax(e.to_string()))?;
        let mut seen = HashSet::new();
        let mut archives = Vec::with_capacity(raw.archive.len());
        for (i, r) in raw.archive.into_iter().enumerate() {
            let a = convert_archive(r, i)?;
            if !seen.insert(a.id.clone()) {
                return Err(ManifestError::Invalid {
                    id: format!("archive {}", a.id),
                    reason: "duplicate id".into(),
                });
            }
            archives.push(a);
        }
        let archive_ids = seen.clone();
        let mut entries = Vec::with_capacity(raw.file.len());
        for (i, r) in raw.file.into_iter().enumerate() {
            let e = convert(r, i)?;
            if !seen.insert(e.id.clone()) {
                return Err(ManifestError::Invalid {
                    id: e.id,
                    reason: "duplicate id (ids are shared with archives)".into(),
                });
            }
            if let Source::Archive { archive, .. } = &e.source
                && !archive_ids.contains(archive)
            {
                return Err(ManifestError::Invalid {
                    reason: format!("archive `{archive}` is not listed"),
                    id: e.id,
                });
            }
            entries.push(e);
        }
        Ok(Self { entries, archives })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn archives(&self) -> &[Archive] {
        &self.archives
    }

    pub fn archive(&self, id: &str) -> Option<&Archive> {
        self.archives.iter().find(|a| a.id == id)
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
        assert!(
            e.contains("flac/subset/01") && e.contains("duplicate"),
            "{e}"
        );
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

    const ARCHIVE: &str = "[[archive]]\nid = \"opus/rfc8251\"\n\
         url = \"https://example.org/v.tar.gz\"\nbytes = 99\nsha256 = \"{HASH}\"\n\
         licence = \"unstated\"\nformat = \"tar.gz\"\n";

    fn archive() -> String {
        ARCHIVE.replace("{HASH}", HASH)
    }

    fn member(extra: &str) -> String {
        format!(
            "[[file]]\nid = \"opus/rfc8251/testvector01.bit\"\nname = \"testvector01.bit\"\n\
             archive = \"opus/rfc8251\"\nmember = \"opus_newvectors/testvector01.bit\"\n\
             bytes = 10\nsha256 = \"{HASH}\"\nlicence = \"unstated\"\nexpect = \"data\"\n{extra}"
        )
    }

    #[test]
    fn expect_data_is_a_reference_file() {
        let t = entry("").replace("\"decode\"", "\"data\"");
        let m = Manifest::parse(&t).unwrap();
        assert_eq!(m.entries()[0].expect, Expect::Data);
        assert_eq!(Expect::Data.as_str(), "data");
    }

    #[test]
    fn an_archive_and_a_member_parse() {
        let m = Manifest::parse(&format!("{}{}", archive(), member(""))).unwrap();
        let a = m.archive("opus/rfc8251").unwrap();
        assert_eq!((a.bytes, a.format), (99, ArchiveFormat::TarGz));
        assert_eq!(a.url, "https://example.org/v.tar.gz");
        assert_eq!(m.archives().len(), 1);
        let e = &m.entries()[0];
        assert_eq!(
            e.source,
            Source::Archive {
                archive: "opus/rfc8251".into(),
                member: "opus_newvectors/testvector01.bit".into()
            }
        );
        assert_eq!(e.url(), None);
        assert_eq!(
            entry_url(&Manifest::parse(&entry("")).unwrap()),
            "https://example.org/a.flac"
        );
    }

    fn entry_url(m: &Manifest) -> &str {
        m.entries()[0].url().unwrap()
    }

    #[test]
    fn a_member_of_an_unknown_archive_is_rejected() {
        let e = err(&member(""));
        assert!(
            e.contains("testvector01.bit") && e.contains("opus/rfc8251"),
            "{e}"
        );
    }

    #[test]
    fn url_and_archive_together_or_neither_are_rejected() {
        let both = member("url = \"https://example.org/x\"\n");
        let e = err(&format!("{}{both}", archive()));
        assert!(e.contains("url") && e.contains("archive"), "{e}");
        let neither = member("").replace("archive = \"opus/rfc8251\"\n", "");
        let e = err(&format!("{}{neither}", archive()));
        assert!(e.contains("member") || e.contains("url"), "{e}");
        let no_member = member("").replace("member = \"opus_newvectors/testvector01.bit\"\n", "");
        assert!(err(&format!("{}{no_member}", archive())).contains("member"));
    }

    #[test]
    fn unsafe_member_names_are_rejected_at_parse() {
        for bad in [
            "../x.bit",
            "a/../x.bit",
            "/etc/passwd",
            "a//x.bit",
            "./x.bit",
            "a\\\\x.bit",
            "",
        ] {
            let t = member("").replace("opus_newvectors/testvector01.bit", bad);
            let e = err(&format!("{}{t}", archive()));
            assert!(
                e.contains("testvector01.bit") && e.contains("member"),
                "{bad:?}: {e}"
            );
        }
    }

    #[test]
    fn archive_keys_are_validated() {
        let dup = format!("{}{}", archive(), archive());
        assert!(err(&dup).contains("duplicate"));
        let bad_hash = archive().replace(HASH, "abc");
        assert!(err(&bad_hash).contains("sha256"));
        let http = archive().replace("https://", "http://");
        assert!(err(&http).contains("https"));
        let fmt = archive().replace("tar.gz", "zip");
        assert!(err(&fmt).contains("zip"));
        let none = archive().replace("bytes = 99\n", "");
        assert!(err(&none).contains("bytes"));
    }

    #[test]
    fn an_archive_id_may_not_collide_with_a_file_id() {
        let e = err(&format!(
            "{}{}",
            archive(),
            entry("").replace("flac/subset/01", "opus/rfc8251")
        ));
        assert!(e.contains("opus/rfc8251"), "{e}");
    }

    #[test]
    fn load_reports_a_missing_file() {
        let e = Manifest::load(Path::new("/nonexistent/corpus.toml"));
        assert!(
            e.unwrap_err()
                .to_string()
                .contains("/nonexistent/corpus.toml")
        );
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
