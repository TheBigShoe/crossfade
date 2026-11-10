//! The `crossfade` binary: all behaviour lives in the library.
#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    crossfade_cli::main()
}
