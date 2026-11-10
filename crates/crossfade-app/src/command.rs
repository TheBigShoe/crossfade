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
            ["convert", "a.wav", "-o", "b.wav", "--sample-format", "s24", "--force"]
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
}
