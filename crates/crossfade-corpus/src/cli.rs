//! The `crossfade-corpus` command line, kept in the library so tests can drive
//! it; `main.rs` only wires in the real downloader and the process exit code.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{Cache, Downloader, Event, Manifest, Outcome, Problem, Received, Summary, fetch, seed};

const USAGE: &str = "usage: crossfade-corpus [--manifest <path>] [--dir <path>] \
<fetch [--only <id-prefix>] | verify | list | seed --from <dir> | path <id>>";

/// Where the manifest and cache live when no flag says otherwise.
#[derive(Debug, Clone)]
pub struct Defaults {
    pub manifest: PathBuf,
    pub dir: PathBuf,
}

#[derive(Default)]
struct Args {
    manifest: Option<PathBuf>,
    dir: Option<PathBuf>,
    only: Option<String>,
    from: Option<PathBuf>,
    positional: Vec<String>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match a.as_str() {
            "--manifest" => parsed.manifest = Some(value(a)?.into()),
            "--dir" => parsed.dir = Some(value(a)?.into()),
            "--only" => parsed.only = Some(value(a)?),
            "--from" => parsed.from = Some(value(a)?.into()),
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            _ => parsed.positional.push(a.clone()),
        }
    }
    Ok(parsed)
}

fn describe(cache: &Cache, entry: &crate::Entry) -> &'static str {
    match cache.verify(entry) {
        Ok(()) => "present",
        Err(Problem::Missing) => "missing",
        Err(_) => "invalid",
    }
}

fn report(summary: &Summary, err: &mut dyn Write) -> i32 {
    for f in &summary.failed {
        let _ = writeln!(err, "FAIL {}: {}", f.id, f.reason);
    }
    let _ = writeln!(
        err,
        "fetched {}, already valid {}, failed {}",
        summary.fetched,
        summary.skipped,
        summary.failed.len()
    );
    i32::from(!summary.is_ok())
}

/// Returns the process exit code: 0 success, 1 something missing or invalid,
/// 2 bad usage.
pub fn run(
    args: &[String],
    defaults: &Defaults,
    downloader: &mut dyn Downloader,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let args = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            let _ = writeln!(err, "{e}\n{USAGE}");
            return 2;
        }
    };
    let Some(command) = args.positional.first().map(String::as_str) else {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    };
    let wanted_args = match command {
        "path" => 2,
        "fetch" | "verify" | "list" | "seed" => 1,
        _ => {
            let _ = writeln!(err, "unknown command `{command}`\n{USAGE}");
            return 2;
        }
    };
    if args.positional.len() != wanted_args || (command == "seed" && args.from.is_none()) {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    }
    let manifest_path = args.manifest.as_deref().unwrap_or(&defaults.manifest);
    let manifest = match Manifest::load(manifest_path) {
        Ok(m) => m,
        Err(e) => {
            let _ = writeln!(err, "{e}");
            return 1;
        }
    };
    let cache = Cache::new(args.dir.clone().unwrap_or_else(|| defaults.dir.clone()));

    match command {
        "fetch" => {
            let summary = fetch(
                &manifest,
                &cache,
                downloader,
                args.only.as_deref(),
                &mut |ev| {
                    let _ = match ev {
                        Event::Starting(e) => {
                            writeln!(err, "fetching {} ({} bytes)", e.id, e.bytes)
                        }
                        Event::Finished(e, Outcome::Skipped) => writeln!(err, "skip {}", e.id),
                        Event::Finished(e, Outcome::Fetched) => writeln!(err, "ok {}", e.id),
                        Event::Finished(e, Outcome::Failed(why)) => {
                            writeln!(err, "FAIL {}: {why}", e.id)
                        }
                    };
                },
            );
            // Failures were already printed per file above.
            let _ = writeln!(
                err,
                "fetched {}, already valid {}, failed {}",
                summary.fetched,
                summary.skipped,
                summary.failed.len()
            );
            i32::from(!summary.is_ok())
        }
        "seed" => {
            let from = args.from.as_deref().unwrap_or(Path::new("."));
            report(&seed(&manifest, &cache, from), err)
        }
        "verify" => {
            let (mut ok, mut missing, mut invalid) = (0, 0, 0);
            for e in manifest.entries() {
                match cache.verify(e) {
                    Ok(()) => ok += 1,
                    Err(p) => {
                        if p == Problem::Missing {
                            missing += 1;
                        } else {
                            invalid += 1;
                        }
                        let _ = writeln!(out, "{}: {p}", e.id);
                    }
                }
            }
            let _ = writeln!(
                out,
                "{ok} present and valid, {missing} missing, {invalid} invalid"
            );
            i32::from(missing + invalid > 0)
        }
        "list" => {
            for e in manifest.entries() {
                let _ = writeln!(
                    out,
                    "{:<18} {:>10} {:<16} {}",
                    e.id,
                    e.bytes,
                    e.expect.as_str(),
                    describe(&cache, e)
                );
            }
            0
        }
        _ => {
            let id = &args.positional[1];
            match crate::try_require(&manifest, &cache, id) {
                Ok(p) => {
                    let _ = writeln!(out, "{}", p.display());
                    0
                }
                Err(e) => {
                    let _ = writeln!(err, "{e}");
                    1
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{entry, temp_dir};
    use std::path::Path;

    struct Fake(Vec<(String, Vec<u8>)>);

    impl Downloader for Fake {
        fn download(&mut self, url: &str, dest: &Path, _max: u64) -> Result<Received, String> {
            let (_, body) = self.0.iter().find(|(u, _)| u == url).ok_or("404")?;
            std::fs::write(dest, body).map_err(|e| e.to_string())?;
            Ok(Received::default())
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
                e.id,
                e.name,
                e.url().unwrap(),
                e.bytes,
                e.sha256
            );
        }
        std::fs::write(root.join("corpus.toml"), text).unwrap();
        Setup {
            defaults: Defaults {
                manifest: root.join("corpus.toml"),
                dir: root.join("cache"),
            },
            a,
            b,
            root,
        }
    }

    fn run_cli(s: &Setup, args: &[&str], dl: &mut dyn Downloader) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args, &s.defaults, dl, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn net(s: &Setup) -> Fake {
        Fake(vec![
            (s.a.url().unwrap().to_string(), b"aaa".to_vec()),
            (s.b.url().unwrap().to_string(), b"bbbb".to_vec()),
        ])
    }

    #[test]
    fn fetch_downloads_everything_and_logs_a_line_per_file() {
        let s = setup("cli-fetch");
        let (code, _, err) = run_cli(&s, &["fetch"], &mut net(&s));
        assert_eq!(code, 0, "{err}");
        assert!(
            err.contains("flac/subset/01") && err.contains("flac/faulty/01"),
            "{err}"
        );
        assert_eq!(run_cli(&s, &["verify"], &mut net(&s)).0, 0);
    }

    #[test]
    fn fetch_only_restricts_by_prefix() {
        let s = setup("cli-only");
        let (code, ..) = run_cli(&s, &["fetch", "--only", "flac/faulty"], &mut net(&s));
        assert_eq!(code, 0);
        let (code, out, _) = run_cli(&s, &["verify"], &mut net(&s));
        assert_eq!(code, 1);
        assert!(
            out.contains("1 present and valid") && out.contains("1 missing"),
            "{out}"
        );
    }

    #[test]
    fn only_may_be_repeated_and_applies_to_verify_list_and_seed() {
        let s = setup("cli-only-many");
        run_cli(&s, &["fetch", "--only", "flac/subset"], &mut net(&s));
        let (code, out, _) = run_cli(&s, &["verify", "--only", "flac/subset"], &mut net(&s));
        assert_eq!(code, 0, "{out}");
        assert!(
            out.contains("1 present and valid") && out.contains("0 missing"),
            "{out}"
        );
        let (code, out, _) = run_cli(
            &s,
            &["verify", "--only", "flac/subset", "--only", "flac/faulty"],
            &mut net(&s),
        );
        assert_eq!(code, 1);
        assert!(out.contains("1 missing"), "{out}");
        let (_, out, _) = run_cli(&s, &["list", "--only", "flac/faulty"], &mut net(&s));
        assert!(
            out.contains("flac/faulty/01") && !out.contains("flac/subset/01"),
            "{out}"
        );
        let from = s.root.join("mirror");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("b.flac"), b"bbbb").unwrap();
        let (code, _, err) = run_cli(
            &s,
            &[
                "seed",
                "--from",
                from.to_str().unwrap(),
                "--only",
                "flac/faulty",
            ],
            &mut Fake(vec![]),
        );
        assert_eq!(code, 0, "{err}");
    }

    #[test]
    fn fetch_logs_the_archive_it_downloads() {
        let s = setup("cli-archive");
        let tgz = crate::testutil::tar_gz(&[("v/a.bit", b"aaa")]);
        let m = crate::testutil::archive_manifest(&tgz, &[("v/a.bit", b"aaa")]);
        let mut text = String::new();
        text += &format!(
            "[[archive]]\nid = \"opus/rfc8251\"\nurl = \"{}\"\nbytes = {}\nsha256 = \"{}\"\n\
             licence = \"x\"\nformat = \"tar.gz\"\n",
            crate::testutil::ARCHIVE_URL,
            m.archives()[0].bytes,
            m.archives()[0].sha256
        );
        let e = &m.entries()[0];
        text += &format!(
            "[[file]]\nid = \"{}\"\nname = \"{}\"\narchive = \"opus/rfc8251\"\nmember = \"v/a.bit\"\n\
             bytes = 3\nsha256 = \"{}\"\nlicence = \"x\"\nexpect = \"data\"\n",
            e.id, e.name, e.sha256
        );
        std::fs::write(&s.defaults.manifest, text).unwrap();
        let mut dl = Fake(vec![(crate::testutil::ARCHIVE_URL.to_string(), tgz)]);
        let (code, _, err) = run_cli(&s, &["fetch"], &mut dl);
        assert_eq!(code, 0, "{err}");
        assert!(err.contains("archive opus/rfc8251"), "{err}");
        assert_eq!(run_cli(&s, &["verify"], &mut dl).0, 0);
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
        assert!(
            out.contains("flac/subset/01") && out.contains("sha256"),
            "{out}"
        );
        assert!(
            out.contains("1 present and valid") && out.contains("1 invalid"),
            "{out}"
        );
    }

    #[test]
    fn seed_copies_from_a_directory() {
        let s = setup("cli-seed");
        let from = s.root.join("mirror");
        for (d, f, data) in [
            ("subset", "a.flac", &b"aaa"[..]),
            ("faulty", "b.flac", b"bbbb"),
        ] {
            std::fs::create_dir_all(from.join(d)).unwrap();
            std::fs::write(from.join(d).join(f), data).unwrap();
        }
        let (code, _, err) = run_cli(
            &s,
            &["seed", "--from", from.to_str().unwrap()],
            &mut Fake(vec![]),
        );
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
            &[
                "--manifest",
                manifest.to_str().unwrap(),
                "--dir",
                other.to_str().unwrap(),
                "fetch",
            ],
            &mut net(&s),
        );
        assert_eq!(code, 0);
        assert!(other.join("flac/subset/01/a.flac").is_file());
        assert!(!s.defaults.dir.exists());
    }

    #[test]
    fn usage_errors_exit_two_and_bad_manifest_exits_one() {
        let s = setup("cli-usage");
        for args in [
            &[][..],
            &["bogus"],
            &["fetch", "--only"],
            &["fetch", "--wat"],
        ] {
            let (code, _, err) = run_cli(&s, args, &mut net(&s));
            assert_eq!(code, 2, "{args:?}");
            assert!(err.contains("usage"), "{err}");
        }
        let (code, _, err) = run_cli(
            &s,
            &["--manifest", "/nonexistent/x.toml", "list"],
            &mut net(&s),
        );
        assert_eq!(code, 1);
        assert!(err.contains("/nonexistent/x.toml"), "{err}");
    }
}
