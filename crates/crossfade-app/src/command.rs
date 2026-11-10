//! What a front end hands to a runner, and the CLI words that mean the same.

use crossfade_graph::Job;

/// A job plus the one decision the job file does not carry.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub job: Job,
    pub overwrite: bool,
}

/// The job has no `crossfade convert` spelling (not one input and one output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotExpressible;

impl std::fmt::Display for NotExpressible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not expressible as a convert command")
    }
}

impl std::error::Error for NotExpressible {}

/// Arguments after `crossfade`. A dash-leading input would be read as a flag,
/// so it moves behind `--`; a dash-leading output uses the attached form.
pub fn convert_args(request: &Request) -> Result<Vec<String>, NotExpressible> {
    let ([input], [output]) = (
        request.job.inputs.as_slice(),
        request.job.outputs.as_slice(),
    ) else {
        return Err(NotExpressible);
    };
    let (input, out_path) = (&input.path, &output.path);
    let dashed_input = input.starts_with('-');
    let mut args = vec!["convert".to_owned()];
    if !dashed_input {
        args.push(input.clone());
    }
    if out_path.starts_with('-') || dashed_input {
        args.push(format!("--output={out_path}"));
    } else {
        args.extend(["-o".to_owned(), out_path.clone()]);
    }
    if let Some(format) = output.audio.sample_format {
        args.extend(["--sample-format".to_owned(), format.name().to_owned()]);
    }
    if let Some(codec) = &output.audio.codec {
        args.extend(["--codec".to_owned(), codec.clone()]);
    }
    if let Some(level) = output.audio.compression_level {
        args.extend(["--compression-level".to_owned(), level.to_string()]);
    }
    if request.overwrite {
        args.push("--force".to_owned());
    }
    if dashed_input {
        args.extend(["--".to_owned(), input.clone()]);
    }
    Ok(args)
}

fn quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// A line a person can paste into a POSIX shell.
pub fn command_line(request: &Request) -> Result<String, NotExpressible> {
    let args = convert_args(request)?;
    let mut line = "crossfade".to_owned();
    for arg in &args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::{NotExpressible, Request, command_line, convert_args};
    use crossfade_core::SampleFormat;
    use crossfade_graph::{AudioSettings, Job, JobInput, JobOutput};

    fn request(input: &str, output: &str, format: Option<SampleFormat>, force: bool) -> Request {
        Request {
            job: Job {
                version: Job::VERSION,
                inputs: vec![JobInput { path: input.into() }],
                outputs: vec![JobOutput {
                    path: output.into(),
                    audio: AudioSettings {
                        sample_format: format,
                        ..AudioSettings::default()
                    },
                }],
            },
            overwrite: force,
        }
    }

    fn args(r: &Request) -> Vec<String> {
        convert_args(r).unwrap()
    }

    #[test]
    fn minimal_request_is_input_and_output() {
        assert_eq!(
            args(&request("a.wav", "b.wav", None, false)),
            ["convert", "a.wav", "-o", "b.wav"]
        );
    }

    #[test]
    fn flags_appear_only_when_set() {
        assert_eq!(
            args(&request("a.wav", "b.wav", Some(SampleFormat::S24), true)),
            [
                "convert",
                "a.wav",
                "-o",
                "b.wav",
                "--sample-format",
                "s24",
                "--force"
            ]
        );
        assert_eq!(
            args(&request("a.wav", "b.wav", Some(SampleFormat::F32), false)),
            ["convert", "a.wav", "-o", "b.wav", "--sample-format", "f32"]
        );
    }

    #[test]
    fn a_job_that_is_not_one_in_one_out_cannot_be_expressed() {
        let mut none = request("a", "b", None, false);
        none.job.inputs.clear();
        assert_eq!(convert_args(&none), Err(NotExpressible));
        let mut two = request("a", "b", None, false);
        two.job.outputs.push(two.job.outputs[0].clone());
        assert_eq!(convert_args(&two), Err(NotExpressible));
        assert_eq!(command_line(&two), Err(NotExpressible));
    }

    #[test]
    fn a_leading_dash_output_uses_the_attached_form() {
        assert_eq!(
            args(&request("a.wav", "-out.wav", None, false)),
            ["convert", "a.wav", "--output=-out.wav"]
        );
    }

    #[test]
    fn a_leading_dash_input_goes_after_a_double_dash() {
        assert_eq!(
            args(&request("-in.wav", "b.wav", Some(SampleFormat::S16), true)),
            [
                "convert",
                "--output=b.wav",
                "--sample-format",
                "s16",
                "--force",
                "--",
                "-in.wav"
            ]
        );
    }

    #[test]
    fn command_line_quotes_only_what_needs_it() {
        let r = request("my song.wav", "it's.wav", Some(SampleFormat::U8), false);
        assert_eq!(
            command_line(&r).unwrap(),
            r#"crossfade convert 'my song.wav' -o 'it'\''s.wav' --sample-format u8"#
        );
        assert_eq!(
            command_line(&request("a/b-c_d.wav", "x.wav", None, false)).unwrap(),
            "crossfade convert a/b-c_d.wav -o x.wav"
        );
    }

    #[test]
    fn an_empty_path_is_quoted() {
        assert_eq!(
            command_line(&request("", "x.wav", None, false)).unwrap(),
            "crossfade convert '' -o x.wav"
        );
    }

    #[test]
    fn codec_and_compression_level_have_flags_after_the_sample_format() {
        let mut r = request("a.wav", "b.flac", Some(SampleFormat::S24), true);
        r.job.outputs[0].audio.codec = Some("flac".into());
        r.job.outputs[0].audio.compression_level = Some(8);
        assert_eq!(
            args(&r),
            [
                "convert",
                "a.wav",
                "-o",
                "b.flac",
                "--sample-format",
                "s24",
                "--codec",
                "flac",
                "--compression-level",
                "8",
                "--force"
            ]
        );
        let mut only_level = request("a.wav", "b.flac", None, false);
        only_level.job.outputs[0].audio.compression_level = Some(0);
        assert_eq!(
            command_line(&only_level).unwrap(),
            "crossfade convert a.wav -o b.flac --compression-level 0"
        );
    }

    #[test]
    fn the_new_flags_go_before_a_double_dash_input() {
        let mut r = request("-in.wav", "b.flac", None, false);
        r.job.outputs[0].audio.codec = Some("copy".into());
        assert_eq!(
            args(&r),
            [
                "convert",
                "--output=b.flac",
                "--codec",
                "copy",
                "--",
                "-in.wav"
            ]
        );
    }
}
