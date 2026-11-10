#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{entry, temp_dir};
    use std::path::Path;

    struct Fake(Vec<(String, Vec<u8>)>);

    impl Downloader for Fake {
        fn download(&mut self, url: &str, dest: &Path) -> Result<(), String> {
            let (_, body) = self.0.iter().find(|(u, _)| u == url).ok_or("404")?;
            std::fs::write(dest, body).map_err(|e| e.to_string())
        }
    }

    struct Setup {
        defaults: Defaults,
        a: crate::Entry,
        b: crate::Entry,
        root: PathBuf,
    }

    fn setup(tag: &str) -> Setup {
        let root = temp_dir(tag);
        let a = entry("flac/subset/01", "a.flac", b"aaa");
        let b = entry("flac/faulty/01", "b.flac", b"bbbb");
        let mut text = String::new();
        for e in [&a, &b] {
            text += &format!(
                "[[file]]\nid = \"{}\"\nname = \"{}\"\nurl = \"{}\"\nbytes = {}\nsha256 = \"{}\"\n\
                 licence = \"CC0-1.0\"\nexpect = \"decode\"\n",
                e.id, e.name, e.url, e.bytes, e.sha256
            );
        }
        std::fs::write(root.join("corpus.toml"), text).unwrap();
        Setup {
            defaults: Defaults { manifest: root.join("corpus.toml"), dir: root.join("cache") },
            a,
            b,
            root,
        }
    }

    fn run_cli(s: &Setup, args: &[&str], dl: &mut dyn Downloader) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args, &s.defaults, dl, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    fn net(s: &Setup) -> Fake {
        Fake(vec![(s.a.url.clone(), b"aaa".to_vec()), (s.b.url.clone(), b"bbbb".to_vec())])
    }

    #[test]
    fn fetch_downloads_everything_and_logs_a_line_per_file() {
        let s = setup("cli-fetch");
        let (code, _, err) = run_cli(&s, &["fetch"], &mut net(&s));
        assert_eq!(code, 0, "{err}");
        assert!(err.contains("flac/subset/01") && err.contains("flac/faulty/01"), "{err}");
        assert_eq!(run_cli(&s, &["verify"], &mut net(&s)).0, 0);
    }

    #[test]
    fn fetch_only_restricts_by_prefix() {
        let s = setup("cli-only");
        let (code, ..) = run_cli(&s, &["fetch", "--only", "flac/faulty"], &mut net(&s));
        assert_eq!(code, 0);
        let (code, out, _) = run_cli(&s, &["verify"], &mut net(&s));
        assert_eq!(code, 1);
        assert!(out.contains("1 present and valid") && out.contains("1 missing"), "{out}");
    }

    #[test]
    fn fetch_failure_exits_one() {
        let s = setup("cli-fetch-fail");
        let (code, _, err) = run_cli(&s, &["fetch"], &mut Fake(vec![]));
        assert_eq!(code, 1);
        assert!(err.contains("404"), "{err}");
    }

    #[test]
    fn list_shows_id_size_expectation_and_presence() {
        let s = setup("cli-list");
        run_cli(&s, &["fetch", "--only", "flac/subset"], &mut net(&s));
        let (code, out, _) = run_cli(&s, &["list"], &mut net(&s));
        assert_eq!(code, 0);
        let line_a = out.lines().find(|l| l.contains("flac/subset/01")).unwrap();
        assert!(line_a.contains('3') && line_a.contains("decode") && line_a.contains("present"));
        let line_b = out.lines().find(|l| l.contains("flac/faulty/01")).unwrap();
        assert!(line_b.contains("missing"), "{line_b}");
    }

    #[test]
    fn verify_names_problems() {
        let s = setup("cli-verify");
        run_cli(&s, &["fetch"], &mut net(&s));
        let p = Cache::new(s.defaults.dir.clone()).path(&s.a);
        std::fs::write(p, b"zzz").unwrap();
        let (code, out, _) = run_cli(&s, &["verify"], &mut net(&s));
        assert_eq!(code, 1);
        assert!(out.contains("flac/subset/01") && out.contains("sha256"), "{out}");
        assert!(out.contains("1 present and valid") && out.contains("1 invalid"), "{out}");
    }

    #[test]
    fn seed_copies_from_a_directory() {
        let s = setup("cli-seed");
        let from = s.root.join("mirror");
        for (d, f, data) in [("subset", "a.flac", &b"aaa"[..]), ("faulty", "b.flac", b"bbbb")] {
            std::fs::create_dir_all(from.join(d)).unwrap();
            std::fs::write(from.join(d).join(f), data).unwrap();
        }
        let (code, _, err) = run_cli(&s, &["seed", "--from", from.to_str().unwrap()], &mut Fake(vec![]));
        assert_eq!(code, 0, "{err}");
        assert_eq!(run_cli(&s, &["verify"], &mut Fake(vec![])).0, 0);
        assert_eq!(run_cli(&s, &["seed"], &mut Fake(vec![])).0, 2);
    }

    #[test]
    fn path_prints_only_verified_files() {
        let s = setup("cli-path");
        assert_eq!(run_cli(&s, &["path", "flac/subset/01"], &mut net(&s)).0, 1);
        run_cli(&s, &["fetch"], &mut net(&s));
        let (code, out, _) = run_cli(&s, &["path", "flac/subset/01"], &mut net(&s));
        assert_eq!(code, 0);
        assert!(out.trim_end().ends_with("flac/subset/01/a.flac"), "{out}");
        assert_eq!(run_cli(&s, &["path", "nope"], &mut net(&s)).0, 1);
        assert_eq!(run_cli(&s, &["path"], &mut net(&s)).0, 2);
    }

    #[test]
    fn overrides_replace_the_defaults() {
        let s = setup("cli-override");
        let other = s.root.join("other-cache");
        let manifest = s.root.join("corpus.toml");
        let (code, ..) = run_cli(
            &s,
            &["--manifest", manifest.to_str().unwrap(), "--dir", other.to_str().unwrap(), "fetch"],
            &mut net(&s),
        );
        assert_eq!(code, 0);
        assert!(other.join("flac/subset/01/a.flac").is_file());
        assert!(!s.defaults.dir.exists());
    }

    #[test]
    fn usage_errors_exit_two_and_bad_manifest_exits_one() {
        let s = setup("cli-usage");
        for args in [&[][..], &["bogus"], &["fetch", "--only"], &["fetch", "--wat"]] {
            let (code, _, err) = run_cli(&s, args, &mut net(&s));
            assert_eq!(code, 2, "{args:?}");
            assert!(err.contains("usage"), "{err}");
        }
        let (code, _, err) = run_cli(&s, &["--manifest", "/nonexistent/x.toml", "list"], &mut net(&s));
        assert_eq!(code, 1);
        assert!(err.contains("/nonexistent/x.toml"), "{err}");
    }
}
