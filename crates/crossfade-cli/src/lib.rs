//! The `crossfade` command-line tool, as a library so tests and front ends can
//! parse its arguments without a process.
#![forbid(unsafe_code)]

mod probe_report;

use clap::builder::PossibleValuesParser;
use clap::{Parser, Subcommand};
use crossfade_app::{FsIo, Request};
use crossfade_core::{JobIo, Limits, SampleFormat};
use crossfade_graph::{AudioSettings, CancelToken, Job, JobInput, JobOutput, probe, run};
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
        /// File to read: wav, flac, aiff/aifc or ogg (FLAC inside).
        input: String,
        /// File to write; its extension picks the container: .wav, .flac, .aiff/.aif/.aifc.
        #[arg(short, long)]
        output: String,
        /// Output sample format; defaults to the input's.
        #[arg(
            long,
            value_parser = PossibleValuesParser::new(["u8", "s16", "s24", "s32", "f32", "f64"])
        )]
        sample_format: Option<String>,
        /// How audio is written: copy the packets unchanged, raw pcm, or flac. Defaults to the
        /// container's own.
        #[arg(long, value_parser = PossibleValuesParser::new(["copy", "pcm", "flac"]))]
        codec: Option<String>,
        /// FLAC effort from 0 to 8: 0 is fastest, 8 gives the smallest file.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=8))]
        compression_level: Option<u8>,
        /// Replace an existing output file.
        #[arg(long)]
        force: bool,
        /// Print the job as JSON and exit without touching any file.
        #[arg(long)]
        emit_job: bool,
    },
}

struct Choices<'a> {
    sample_format: Option<&'a str>,
    codec: Option<&'a str>,
    compression_level: Option<u8>,
}

fn convert_job(input: String, output: String, choices: Choices<'_>) -> Job {
    Job {
        version: Job::VERSION,
        inputs: vec![JobInput { path: input }],
        outputs: vec![JobOutput {
            path: output,
            audio: AudioSettings {
                // The parser only lets valid names through.
                sample_format: choices.sample_format.and_then(SampleFormat::from_name),
                codec: choices.codec.map(str::to_owned),
                compression_level: choices.compression_level,
            },
        }],
    }
}

/// Parses arguments (without `argv[0]`): `Some` for `convert`, `None` for any
/// other subcommand. This is what a front end checks its own job against.
pub fn request_from_args<I, S>(args: I) -> Result<Option<Request>, clap::Error>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString> + Clone,
{
    let argv = std::iter::once("crossfade".into()).chain(args.into_iter().map(Into::into));
    Ok(match Cli::try_parse_from(argv)?.command {
        Command::Convert {
            input,
            output,
            sample_format,
            codec,
            compression_level,
            force,
            ..
        } => Some(Request {
            job: convert_job(
                input,
                output,
                Choices {
                    sample_format: sample_format.as_deref(),
                    codec: codec.as_deref(),
                    compression_level,
                },
            ),
            overwrite: force,
        }),
        Command::Probe { .. } => None,
    })
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
            codec,
            compression_level,
            force,
            emit_job,
        } => {
            let choices = Choices {
                sample_format: sample_format.as_deref(),
                codec: codec.as_deref(),
                compression_level,
            };
            let job = convert_job(input, output.clone(), choices);
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

pub fn main() -> ExitCode {
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
