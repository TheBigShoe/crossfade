//! The `crossfade` command-line tool.
#![forbid(unsafe_code)]

mod probe_report;

use clap::builder::PossibleValuesParser;
use clap::{Parser, Subcommand};
use crossfade_core::{JobIo, Limits, SampleFormat};
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, probe, run};
use crossfade_app::FsIo;
use std::path::Path;
use std::process::ExitCode;

/// An FFmpeg-class audio/video tool in pure Rust.
#[derive(Parser)]
#[command(
    name = "crossfade",
    bin_name = "crossfade",
    version,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show what a media file contains.
    Probe {
        input: String,
        /// Machine-readable output (schema 1).
        #[arg(long)]
        json: bool,
    },
    /// Convert a media file.
    Convert {
        input: String,
        #[arg(short, long)]
        output: String,
        /// Output sample format; defaults to the input's.
        #[arg(
            long,
            value_parser = PossibleValuesParser::new(["u8", "s16", "s24", "s32", "f32", "f64"])
        )]
        sample_format: Option<String>,
        /// Replace an existing output file.
        #[arg(long)]
        force: bool,
        /// Print the job as JSON and exit without touching any file.
        #[arg(long)]
        emit_job: bool,
    },
}

fn execute(command: Command, io: &mut dyn JobIo) -> Result<String, String> {
    match command {
        Command::Probe { input, json } => {
            let report = probe(&input, io, &Limits::default()).map_err(|e| e.to_string())?;
            Ok(if json {
                format!("{}\n", probe_report::to_json(&report))
            } else {
                probe_report::to_text(&report)
            })
        }
        Command::Convert {
            input,
            output,
            sample_format,
            force,
            emit_job,
        } => {
            let job = Job {
                version: Job::VERSION,
                inputs: vec![JobInput { path: input }],
                outputs: vec![JobOutput {
                    path: output.clone(),
                    audio: AudioSettings {
                        // The parser only lets valid names through.
                        sample_format: sample_format.as_deref().and_then(SampleFormat::from_name),
                    },
                }],
            };
            if emit_job {
                return Ok(format!("{}\n", job.to_json_pretty()));
            }
            if !force && Path::new(&output).exists() {
                return Err(format!(
                    "{output} already exists (use --force to replace it)"
                ));
            }
            run(
                &job,
                io,
                &Limits::default(),
                &CancelToken::new(),
                &mut |_| {},
            )
            .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
    }
}

fn main() -> ExitCode {
    // clap prints help/usage to stderr and picks exit code 2 for both the
    // no-argument and the bad-flag case; --version goes to stdout with 0.
    let cli = Cli::parse();
    match execute(cli.command, &mut FsIo::new()) {
        Ok(stdout) => {
            print!("{stdout}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("crossfade: {message}");
            ExitCode::from(1)
        }
    }
}
