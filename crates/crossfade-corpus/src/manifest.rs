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
