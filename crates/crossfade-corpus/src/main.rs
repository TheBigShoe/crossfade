#![forbid(unsafe_code)]

use std::process::ExitCode;

use crossfade_corpus::{Cache, CurlDownloader, Defaults, run, workspace_root};

fn main() -> ExitCode {
    let root = workspace_root();
    let defaults = Defaults {
        manifest: root.join("corpus.toml"),
        dir: Cache::dir_from_env(&root),
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(
        &args,
        &defaults,
        &mut CurlDownloader,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    );
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
