#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{entry, temp_dir};
    use crate::Manifest;

    fn setup(tag: &str, data: &[u8]) -> (Manifest, Cache, crate::Entry) {
        let e = entry("flac/subset/01", "a.flac", data);
        let m = Manifest::parse(&format!(
            "[[file]]\nid = \"{}\"\nname = \"{}\"\nurl = \"{}\"\nbytes = {}\nsha256 = \"{}\"\n\
             licence = \"CC0-1.0\"\nexpect = \"decode\"\n",
            e.id, e.name, e.url, e.bytes, e.sha256
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
        let msg = try_require(&m, &c, "flac/subset/01").unwrap_err().to_string();
        assert!(msg.contains("just corpus") && msg.contains("sha256"), "{msg}");
    }

    #[test]
    fn unknown_id_is_an_error() {
        let (m, c, _) = setup("req-unknown", b"abc");
        let msg = try_require(&m, &c, "flac/nope/99").unwrap_err().to_string();
        assert!(msg.contains("flac/nope/99") && msg.contains("corpus.toml"), "{msg}");
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
