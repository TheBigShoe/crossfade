//! The `crossfade` command-line tool, as a library so tests and front ends can
//! parse its arguments without a process.
#![forbid(unsafe_code)]

mod probe_report;

use clap::builder::PossibleValuesParser;
use clap::{Parser, Subcommand};
use crossfade_app::{FsIo, Request};
use crossfade_core::{Error, JobIo, Limits, SampleFormat};
use crossfade_graph::{AudioSettings, CancelToken, Event, Job, JobInput, JobOutput, probe, run};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Exit code of a run stopped by an interrupt: 128 plus SIGINT, the shell convention.
pub const EXIT_CANCELLED: u8 = 130;

/// How a command ended.
#[derive(Debug)]
pub enum Outcome {
    /// What to print on stdout.
    Done(String),
    Failed(String),
    Cancelled,
}

impl Outcome {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Done(_) => 0,
            Self::Failed(_) => 1,
            Self::Cancelled => EXIT_CANCELLED,
        }
    }
}

fn execute(
    command: Command,
    io: &mut dyn JobIo,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(&Event),
) -> Outcome {
    match command {
        Command::Probe { input, json } => match probe(&input, io, &Limits::default()) {
            Ok(report) => Outcome::Done(if json {
                format!("{}\n", probe_report::to_json(&report))
            } else {
                probe_report::to_text(&report)
            }),
            Err(e) => Outcome::Failed(e.to_string()),
        },
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
                return Outcome::Done(format!("{}\n", job.to_json_pretty()));
            }
            if !force && Path::new(&output).exists() {
                return Outcome::Failed(format!(
                    "{output} already exists (use --force to replace it)"
                ));
            }
            match run(&job, io, &Limits::default(), cancel, on_event) {
                Ok(()) => Outcome::Done(String::new()),
                Err(Error::Cancelled) => Outcome::Cancelled,
                Err(e) => Outcome::Failed(e.to_string()),
            }
        }
    }
}

/// Runs a command line (without `argv[0]`) against `io`. `cancel` stops a conversion
/// and `on_event` sees its progress; both exist so tests can interrupt without a signal.
pub fn run_command<I, S>(
    args: I,
    io: &mut dyn JobIo,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(&Event),
) -> Result<Outcome, clap::Error>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString> + Clone,
{
    let argv = std::iter::once("crossfade".into()).chain(args.into_iter().map(Into::into));
    Ok(execute(
        Cli::try_parse_from(argv)?.command,
        io,
        cancel,
        on_event,
    ))
}

pub fn main() -> ExitCode {
    // clap prints help/usage to stderr and picks exit code 2 for both the
    // no-argument and the bad-flag case; --version goes to stdout with 0.
    let cli = Cli::parse();
    let cancel = CancelToken::new();
    // Only a conversion has anything to cancel; elsewhere the default (die) is right.
    if matches!(cli.command, Command::Convert { .. }) {
        install_interrupt_handler(&cancel);
    }
    let outcome = execute(cli.command, &mut FsIo::new(), &cancel, &mut |_| {});
    match &outcome {
        Outcome::Done(stdout) => print!("{stdout}"),
        Outcome::Failed(message) => eprintln!("crossfade: {message}"),
        Outcome::Cancelled => eprintln!("crossfade: cancelled"),
    }
    ExitCode::from(outcome.exit_code())
}

/// SIGINT and SIGTERM (Ctrl-C and Ctrl-Break on Windows) cancel the conversion so the
/// engine discards its partial output. The handler runs on its own thread, not in signal
/// context; a second interrupt exits at once so a stuck process can still be killed.
fn install_interrupt_handler(cancel: &CancelToken) {
    let cancel = cancel.clone();
    let seen = AtomicBool::new(false);
    let installed = ctrlc::set_handler(move || {
        if seen.swap(true, Ordering::SeqCst) {
            std::process::exit(i32::from(EXIT_CANCELLED));
        }
        cancel.cancel();
    });
    // Without a handler Ctrl-C still kills the process, as before.
    if let Err(e) = installed {
        eprintln!("crossfade: cannot catch interrupts: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::{EXIT_CANCELLED, Outcome, run_command};
    use crossfade_app::FsIo;
    use crossfade_graph::{CancelToken, Event};
    use std::path::PathBuf;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("crossfade-cli-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }

        fn path(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }

        fn listing(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Mono 16-bit 8 kHz silence; large enough for the engine to report progress
    /// several times.
    fn write_silence(dir: &Dir, name: &str, frames: u32) -> String {
        let data = frames * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16_000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        v.resize(v.len() + data as usize, 0);
        let path = dir.path(name);
        std::fs::write(&path, v).unwrap();
        path
    }

    fn convert(dir: &Dir, cancel: &CancelToken, on_event: &mut dyn FnMut(&Event)) -> Outcome {
        let args = ["convert", &dir.path("in.wav"), "-o", &dir.path("out.flac")];
        run_command(args, &mut FsIo::new(), cancel, on_event).unwrap()
    }

    #[test]
    fn a_token_cancelled_before_start_leaves_only_the_input() {
        let d = Dir::new("pre");
        write_silence(&d, "in.wav", 1000);
        let cancel = CancelToken::new();
        cancel.cancel();
        let outcome = convert(&d, &cancel, &mut |_| {});
        assert!(matches!(outcome, Outcome::Cancelled));
        assert_eq!(outcome.exit_code(), EXIT_CANCELLED);
        assert_eq!(EXIT_CANCELLED, 130);
        assert_eq!(d.listing(), ["in.wav"]);
    }

    #[test]
    fn cancelling_from_a_progress_report_stops_and_cleans_up() {
        let d = Dir::new("mid");
        write_silence(&d, "in.wav", 2_000_000);
        let cancel = CancelToken::new();
        let mut reports = 0;
        let outcome = convert(&d, &cancel, &mut |event| {
            if matches!(event, Event::Progress { .. }) {
                reports += 1;
                cancel.cancel();
            }
        });
        assert!(reports >= 1, "the conversion never reported progress");
        assert!(matches!(outcome, Outcome::Cancelled));
        assert_eq!(outcome.exit_code(), 130);
        assert_eq!(d.listing(), ["in.wav"]);
    }

    #[test]
    fn an_uncancelled_run_succeeds_and_failures_exit_1() {
        let d = Dir::new("ok");
        write_silence(&d, "in.wav", 1000);
        let outcome = convert(&d, &CancelToken::new(), &mut |_| {});
        assert!(matches!(outcome, Outcome::Done(_)));
        assert_eq!(outcome.exit_code(), 0);
        assert_eq!(d.listing(), ["in.wav", "out.flac"]);
        let again = convert(&d, &CancelToken::new(), &mut |_| {});
        assert!(
            matches!(again, Outcome::Failed(_)),
            "exists without --force"
        );
        assert_eq!(again.exit_code(), 1);
    }
}
