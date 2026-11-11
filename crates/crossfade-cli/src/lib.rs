//! The `crossfade` command-line tool, as a library so tests and front ends can
//! parse its arguments without a process.
#![forbid(unsafe_code)]

mod probe_report;

use clap::builder::PossibleValuesParser;
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand};
use crossfade_app::{FsIo, Request};
use crossfade_core::{Error, JobIo, Limits, SampleFormat};
use crossfade_graph::filter::{
    Channels, Curve, DEFAULT_TRUE_PEAK_DBTP, FilterSpec, parse_channels, parse_db, parse_dbtp,
    parse_lufs, parse_rate, parse_time,
};
use crossfade_graph::{
    AudioSettings, CancelToken, Dither, Event, Job, JobInput, JobOutput, Normalization,
    ProbeOptions, probe_with_cancel, run,
};
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

/// How shortcuts and filters combine, shown under `convert --help` and `concat --help`.
const PROCESSING_HELP: &str = "\
Processing order: --start/--end/--duration (one trim), then each --filter in the order given, \
then --fade-in/--fade-out (one fade), --gain, --normalize; last, the conversions --sample-rate \
and --channels ask for. The job lists the filters in that order.

Filters: gain <dB> | trim start=<time> end=<time> duration=<time> | fade in=<time> out=<time> \
curve=linear|equal-power|s-curve | normalize <LUFS> peak=<dBTP> | resample <rate> \
quality=fast|good|best | mix <layout or count> matrix=<rows>
Units are written: 10s, 500ms, 1:30, 1:02:03.5; -3dB; -16LUFS; -1dBTP; 48000, 48000Hz, 44.1kHz.
Layouts: mono, stereo, 2.1, 3.0, 3.1, 4.0, quad, quad-side, 5.0, 5.0-back, 5.1, 5.1-back, 6.1, \
7.1.";

const CONCAT_HELP: &str = "\
Every input is conformed to the first input's sample rate and channel layout (or to \
--sample-rate and --channels), then they are joined in order and the filters run on the joined \
stream. `crossfade convert` takes exactly one input, so a shell glob never joins files by \
accident.";

#[derive(Subcommand)]
enum Command {
    /// Show what a media file contains.
    Probe {
        input: String,
        /// Machine-readable output (schema 1).
        #[arg(long)]
        json: bool,
        /// Also decode the file and measure loudness (LUFS, LU range) and peaks (dBTP, dBFS).
        #[arg(long)]
        loudness: bool,
    },
    /// Convert a media file.
    #[command(after_help = PROCESSING_HELP)]
    Convert {
        /// File to read: wav, flac, aiff/aifc or ogg (FLAC inside).
        input: String,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Join media files end to end into one.
    #[command(after_help = format!("{CONCAT_HELP}\n\n{PROCESSING_HELP}"))]
    Concat {
        /// Files to read, in order (at least two): wav, flac, aiff/aifc or ogg (FLAC inside).
        #[arg(required = true, num_args = 2..)]
        inputs: Vec<String>,
        #[command(flatten)]
        out: OutputArgs,
    },
}

/// What `convert` and `concat` share: where to write, how, and the processing.
#[derive(Args)]
struct OutputArgs {
    /// File to write; its extension picks the container: .wav, .flac, .aiff/.aif/.aifc, .oga/.ogg (Ogg FLAC).
    #[arg(short, long)]
    output: String,
    /// Output sample format; defaults to the input's.
    #[arg(
        long,
        value_parser = PossibleValuesParser::new(["u8", "s16", "s24", "s32", "f32", "f64"])
    )]
    sample_format: Option<String>,
    /// Output sample rate, such as 48000 or 44.1kHz; defaults to the input's. Converted after
    /// the filters.
    #[arg(long, value_name = "HZ", value_parser = value(parse_rate))]
    sample_rate: Option<u32>,
    /// Output channels: a layout (mono, stereo, 5.1, ...) or a count; defaults to the input's.
    /// Converted after the filters.
    #[arg(long, value_name = "LAYOUT", value_parser = value(parse_channels))]
    channels: Option<Channels>,
    /// How audio is written: copy the packets unchanged, raw pcm, or flac. Defaults to the
    /// container's own.
    #[arg(long, value_parser = PossibleValuesParser::new(["copy", "pcm", "flac"]))]
    codec: Option<String>,
    /// FLAC effort from 0 to 8: 0 is fastest, 8 gives the smallest file.
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=8))]
    compression_level: Option<u8>,
    /// A filter: a name, an optional value, then key=value words, such as "gain -3dB" or
    /// "fade out=3s curve=equal-power". Repeat for more; they run in the order given.
    #[arg(long = "filter", value_name = "FILTER", value_parser = value(str::parse::<FilterSpec>), help_heading = "Processing")]
    filters: Vec<FilterSpec>,
    /// Change the level, such as -6dB (not with --normalize, which sets the level itself).
    #[arg(long, value_name = "dB", allow_hyphen_values = true, value_parser = value(parse_db), conflicts_with = "normalize", help_heading = "Processing")]
    gain: Option<f64>,
    /// Start at this time, such as 10s or 1:30.
    #[arg(long, value_name = "TIME", value_parser = value(parse_time), help_heading = "Processing")]
    start: Option<f64>,
    /// End at this time.
    #[arg(long, value_name = "TIME", value_parser = value(parse_time), conflicts_with = "duration", help_heading = "Processing")]
    end: Option<f64>,
    /// Keep this long from the start.
    #[arg(long, value_name = "TIME", value_parser = value(parse_time), help_heading = "Processing")]
    duration: Option<f64>,
    /// Fade in from silence over this long.
    #[arg(long, value_name = "TIME", value_parser = value(parse_time), help_heading = "Processing")]
    fade_in: Option<f64>,
    /// Fade out to silence over this long, ending at the end.
    #[arg(long, value_name = "TIME", value_parser = value(parse_time), help_heading = "Processing")]
    fade_out: Option<f64>,
    /// Bring the integrated loudness to this target, such as -16LUFS (measures the whole
    /// programme first).
    #[arg(long, value_name = "LUFS", allow_hyphen_values = true, value_parser = value(parse_lufs), help_heading = "Processing")]
    normalize: Option<f64>,
    /// The true peak --normalize may not pass [default: -1dBTP].
    #[arg(long, value_name = "dBTP", allow_hyphen_values = true, value_parser = value(parse_dbtp), requires = "normalize", help_heading = "Processing")]
    true_peak: Option<f64>,
    /// How processed audio is rounded to an integer format: tpdf (triangular dither, the
    /// default) or none. Only used when something is processed; float output is never
    /// dithered.
    #[arg(long, value_parser = PossibleValuesParser::new(Dither::NAMES), help_heading = "Processing")]
    dither: Option<String>,
    /// Threads for coding: auto (the default) uses the machine's cores, 1 keeps everything
    /// on one thread. The output is the same either way.
    #[arg(long, default_value = "auto", value_parser = parse_threads)]
    threads: usize,
    /// Replace an existing output file.
    #[arg(long)]
    force: bool,
    /// Print the job as JSON and exit without touching any file.
    #[arg(long)]
    emit_job: bool,
}

/// A value parser from one of the filter word parsers, keeping its message.
fn value<T, E: ToString>(
    f: impl Fn(&str) -> Result<T, E> + Clone + Send + Sync + 'static,
) -> impl Fn(&str) -> Result<T, String> + Clone + Send + Sync + 'static {
    move |s| f(s).map_err(|e| e.to_string())
}

/// Most threads `--threads` takes; the encoder uses no more than this anyway.
const MAX_THREADS: usize = 64;

/// `auto` is 0, the engine's "the machine's parallelism".
fn parse_threads(s: &str) -> Result<usize, String> {
    if s == "auto" {
        return Ok(0);
    }
    match s.parse::<usize>() {
        Ok(n) if (1..=MAX_THREADS).contains(&n) => Ok(n),
        _ => Err(format!("expected auto or 1 to {MAX_THREADS}")),
    }
}

impl OutputArgs {
    /// The ordered filter list: the trim shortcuts, every `--filter`, then the fade, gain and
    /// normalise shortcuts (see `PROCESSING_HELP`). A shortcut's value is checked here, as its
    /// filter is.
    fn filters(&self) -> Result<Vec<FilterSpec>, String> {
        let mut shortcuts = Vec::new();
        if self.start.is_some() || self.end.is_some() || self.duration.is_some() {
            shortcuts.push((
                "--start/--end/--duration",
                FilterSpec::Trim {
                    start_seconds: self.start,
                    end_seconds: self.end,
                    duration_seconds: self.duration,
                },
            ));
        }
        let first_after = shortcuts.len();
        if self.fade_in.is_some() || self.fade_out.is_some() {
            shortcuts.push((
                "--fade-in/--fade-out",
                FilterSpec::Fade {
                    in_seconds: self.fade_in,
                    out_seconds: self.fade_out,
                    curve: Curve::Linear,
                },
            ));
        }
        if let Some(db) = self.gain {
            shortcuts.push(("--gain", FilterSpec::Gain { db }));
        }
        if let Some(lufs) = self.normalize {
            shortcuts.push((
                "--normalize",
                FilterSpec::Normalize {
                    lufs,
                    true_peak_dbtp: self.true_peak.unwrap_or(DEFAULT_TRUE_PEAK_DBTP),
                },
            ));
        }
        for (flags, f) in &shortcuts {
            f.check().map_err(|e| format!("{flags}: {e}"))?;
        }
        let mut shortcuts = shortcuts.into_iter().map(|(_, f)| f);
        let mut all: Vec<FilterSpec> = shortcuts.by_ref().take(first_after).collect();
        all.extend(self.filters.iter().cloned());
        all.extend(shortcuts);
        Ok(all)
    }

    fn settings(&self) -> Result<AudioSettings, String> {
        Ok(AudioSettings {
            // The parsers only let valid names through.
            sample_format: self
                .sample_format
                .as_deref()
                .and_then(SampleFormat::from_name),
            codec: self.codec.clone(),
            compression_level: self.compression_level,
            sample_rate: self.sample_rate,
            channels: self.channels.clone(),
            dither: self
                .dither
                .as_deref()
                .and_then(Dither::from_name)
                .unwrap_or_default(),
            filters: self.filters()?,
        })
    }

    fn job(&self, inputs: Vec<String>) -> Result<Job, String> {
        Ok(Job {
            version: Job::VERSION,
            inputs: inputs.into_iter().map(|path| JobInput { path }).collect(),
            outputs: vec![JobOutput {
                path: self.output.clone(),
                audio: self.settings()?,
            }],
        })
    }
}

/// A job's arguments that clap cannot check alone, refused as clap refuses a bad value.
fn invalid(message: String) -> clap::Error {
    Cli::command().error(ErrorKind::ValueValidation, message)
}

/// Parses arguments (without `argv[0]`): `Some` for `convert` and `concat`, `None` for any
/// other subcommand. This is what a front end checks its own job against.
pub fn request_from_args<I, S>(args: I) -> Result<Option<Request>, clap::Error>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString> + Clone,
{
    let argv = std::iter::once("crossfade".into()).chain(args.into_iter().map(Into::into));
    let (inputs, out) = match Cli::try_parse_from(argv)?.command {
        Command::Convert { input, out } => (vec![input], out),
        Command::Concat { inputs, out } => (inputs, out),
        Command::Probe { .. } => return Ok(None),
    };
    Ok(Some(Request {
        job: out.job(inputs).map_err(invalid)?,
        overwrite: out.force,
    }))
}

/// What the CLI prints, after `crossfade: note: `, about a normalisation: `None` when it
/// reached its target.
pub fn normalize_note(n: &Normalization) -> Option<String> {
    let Some(reached) = n.reached_lufs() else {
        return Some(
            "nothing to normalise (silence, or under 400 ms): the level is left as it is".into(),
        );
    };
    n.limited_by_ceiling.then(|| {
        format!(
            "normalised to {reached:.1} LUFS, {:.1} LU short of {} LUFS: the true peak may not \
             pass {} dBTP (lower the target or raise --true-peak)",
            n.target_lufs - reached,
            n.target_lufs,
            n.ceiling_dbtp
        )
    })
}

/// What the CLI prints, after `crossfade: warning: `, when a run saturated samples.
pub fn clip_warning(samples: u64) -> String {
    let (noun, pronoun) = if samples == 1 {
        ("sample", "it")
    } else {
        ("samples", "them")
    };
    format!(
        "{samples} {noun} clipped: the output format cannot hold {pronoun}. Lower the level \
         (for example --gain -3dB) or write floating point (--sample-format f32)"
    )
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
        Command::Probe {
            input,
            json,
            loudness,
        } => match probe_with_cancel(
            &input,
            io,
            &Limits::default(),
            &ProbeOptions { loudness },
            cancel,
        ) {
            Ok(report) => Outcome::Done(if json {
                format!("{}\n", probe_report::to_json(&report))
            } else {
                probe_report::to_text(&report)
            }),
            Err(Error::Cancelled) => Outcome::Cancelled,
            Err(e) => Outcome::Failed(e.to_string()),
        },
        Command::Convert { input, out } => convert(vec![input], &out, io, cancel, on_event),
        Command::Concat { inputs, out } => convert(inputs, &out, io, cancel, on_event),
    }
}

fn convert(
    inputs: Vec<String>,
    out: &OutputArgs,
    io: &mut dyn JobIo,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(&Event),
) -> Outcome {
    let job = match out.job(inputs) {
        Ok(job) => job,
        Err(e) => return Outcome::Failed(e),
    };
    if out.emit_job {
        return Outcome::Done(format!("{}\n", job.to_json_pretty()));
    }
    let output = &out.output;
    if !out.force && Path::new(output).exists() {
        return Outcome::Failed(format!(
            "{output} already exists (use --force to replace it)"
        ));
    }
    // How many threads is the run's business, not the job's.
    let limits = Limits {
        max_threads: out.threads,
        ..Limits::default()
    };
    match run(&job, io, &limits, cancel, on_event) {
        Ok(()) => Outcome::Done(String::new()),
        Err(Error::Cancelled) => Outcome::Cancelled,
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// Refuses, as a usage error, what only the whole job shows (a shortcut's value out of range).
fn check_job(command: &Command) -> Result<(), clap::Error> {
    match command {
        Command::Convert { out, .. } | Command::Concat { out, .. } => {
            out.settings().map(|_| ()).map_err(invalid)
        }
        Command::Probe { .. } => Ok(()),
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
    let command = Cli::try_parse_from(argv)?.command;
    check_job(&command)?;
    Ok(execute(command, io, cancel, on_event))
}

pub fn main() -> ExitCode {
    // clap prints help/usage to stderr and picks exit code 2 for both the
    // no-argument and the bad-flag case; --version goes to stdout with 0.
    let cli = Cli::parse();
    if let Err(e) = check_job(&cli.command) {
        e.exit();
    }
    let cancel = CancelToken::new();
    // Only a conversion has anything to cancel; elsewhere the default (die) is right.
    if matches!(
        cli.command,
        Command::Convert { .. } | Command::Concat { .. }
    ) {
        install_interrupt_handler(&cancel);
    }
    let mut on_event = |e: &Event| match e {
        Event::Clipped { samples } => eprintln!("crossfade: warning: {}", clip_warning(*samples)),
        Event::Started { plan } => {
            for note in &plan.notes {
                eprintln!("crossfade: note: {note}");
            }
        }
        Event::Normalized(n) => {
            if let Some(note) = normalize_note(n) {
                eprintln!("crossfade: note: {note}");
            }
        }
        _ => {}
    };
    let outcome = execute(cli.command, &mut FsIo::new(), &cancel, &mut on_event);
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
    use super::{EXIT_CANCELLED, Outcome, request_from_args, run_command};
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
    fn cancelling_an_ogg_conversion_leaves_nothing_behind() {
        let d = Dir::new("mid-ogg");
        write_silence(&d, "in.wav", 2_000_000);
        let cancel = CancelToken::new();
        let args = ["convert", &d.path("in.wav"), "-o", &d.path("out.oga")];
        let outcome = run_command(args, &mut FsIo::new(), &cancel, &mut |event| {
            if matches!(event, Event::Progress { .. }) {
                cancel.cancel();
            }
        })
        .unwrap();
        assert!(matches!(outcome, Outcome::Cancelled));
        assert_eq!(d.listing(), ["in.wav"]);
    }

    #[test]
    fn the_thread_count_is_not_part_of_the_job() {
        let base = [
            "convert",
            "a.wav",
            "-o",
            "b.flac",
            "--compression-level",
            "8",
        ];
        let plain = request_from_args(base).unwrap();
        for threads in ["1", "4", "64", "auto"] {
            let with = request_from_args(base.iter().copied().chain(["--threads", threads]));
            assert_eq!(with.unwrap(), plain, "--threads {threads}");
        }
        for bad in ["0", "65", "x", "-1", ""] {
            assert!(
                request_from_args(base.iter().copied().chain(["--threads", bad])).is_err(),
                "--threads {bad:?}"
            );
        }
    }

    #[test]
    fn emit_job_says_nothing_about_threads() {
        let d = Dir::new("emit");
        let emit = |extra: &[&str]| {
            let mut args = vec!["convert", "a.wav", "-o", "b.flac", "--emit-job"];
            args.extend_from_slice(extra);
            match run_command(args, &mut FsIo::new(), &CancelToken::new(), &mut |_| {}).unwrap() {
                Outcome::Done(json) => json,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(emit(&["--threads", "3"]), emit(&[]));
        assert!(!emit(&["--threads", "3"]).contains("thread"));
        assert_eq!(d.listing(), Vec::<String>::new());
    }

    #[test]
    fn every_thread_count_writes_the_same_file() {
        let d = Dir::new("threads");
        let input = write_silence(&d, "in.wav", 50_000);
        let mut outputs = Vec::new();
        for threads in ["1", "2", "auto"] {
            let out = d.path(&format!("out{threads}.flac"));
            let args = ["convert", &input, "-o", &out, "--threads", threads];
            let outcome = run_command(args, &mut FsIo::new(), &CancelToken::new(), &mut |_| {});
            assert!(matches!(outcome.unwrap(), Outcome::Done(_)), "{threads}");
            outputs.push(std::fs::read(&out).unwrap());
        }
        assert!(outputs.windows(2).all(|w| w[0] == w[1]));
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
