//! The `crossfade` command-line tool.
#![forbid(unsafe_code)]

mod fsio;

use clap::Parser;
use std::process::ExitCode;

/// An FFmpeg-class audio/video tool in pure Rust.
#[derive(Parser)]
#[command(
    name = "crossfade",
    bin_name = "crossfade",
    version,
    arg_required_else_help = true
)]
struct Cli {}

fn main() -> ExitCode {
    // clap prints help/usage to stderr and picks exit code 2 for both the
    // no-argument and the bad-flag case; --version goes to stdout with 0.
    let _cli = Cli::parse();
    ExitCode::SUCCESS
}
