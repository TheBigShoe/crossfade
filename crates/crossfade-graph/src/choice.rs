//! Which codec a job writes and which containers can hold which codecs.

use crossfade_codec::EncodeOptions;
use crossfade_core::{CodecId, Error, SampleFormat};
use crossfade_format::FormatId;

fn unsupported(what: impl Into<String>) -> Error {
    Error::Unsupported { what: what.into() }
}

/// The codec to encode with for `format`. `requested` is the job's codec name; `None` is the
/// container's default. `"copy"` never reaches here: the pipeline handles it first.
pub fn output_codec(
    format: FormatId,
    sample_format: SampleFormat,
    requested: Option<&str>,
) -> Result<CodecId, Error> {
    let refuse = |codec: &str| {
        unsupported(format!(
            "codec {codec} cannot be written into {}",
            format.name()
        ))
    };
    let accept = |name: &str| match requested {
        Some(r) if r != name => Err(refuse(r)),
        _ => Ok(()),
    };
    match format {
        FormatId::Wav => accept("pcm").map(|()| CodecId::pcm_for(sample_format)),
        FormatId::Aiff => accept("pcm").map(|()| CodecId::pcm_be_for(sample_format)),
        FormatId::Flac => {
            accept("flac")?;
            if sample_format.is_float() {
                return Err(unsupported(
                    "flac cannot store floating-point samples; choose --sample-format s16, s24 or s32",
                ));
            }
            Ok(CodecId::Flac)
        }
        // Ogg, and any container added later until it is wired here.
        _ => Err(unsupported(format!(
            "writing {} is not supported yet",
            format.name()
        ))),
    }
}

/// Containers we can write; a reader existing for one does not make it writable.
pub fn can_write(format: FormatId) -> bool {
    matches!(format, FormatId::Wav | FormatId::Flac | FormatId::Aiff)
}

/// File extensions a front end can offer for an output, canonical spelling first.
pub fn writable_extensions() -> &'static [&'static str] {
    &["wav", "flac", "aiff", "aif", "aifc"]
}

/// Whether `format` can carry `codec`'s packets unchanged (stream copy).
pub fn can_hold(format: FormatId, codec: CodecId) -> bool {
    use CodecId::*;
    match format {
        FormatId::Wav => matches!(
            codec,
            PcmU8 | PcmS16Le | PcmS24Le | PcmS32Le | PcmF32Le | PcmF64Le
        ),
        FormatId::Aiff => matches!(
            codec,
            PcmS8 | PcmS16Be | PcmS24Be | PcmS32Be | PcmF32Be | PcmF64Be
        ),
        FormatId::Flac => codec == Flac,
        _ => false,
    }
}

/// Threads for the decoder and the encoder out of a run's `budget` (at least 1; 0, "the
/// machine", is resolved before). A codec that cannot use threads gets 1; the other gets the
/// whole budget. When both can (FLAC to FLAC) the decoder, several times cheaper per sample,
/// gets a quarter but at least two where the budget allows (its MD5 then has a thread of its
/// own instead of the caller's), and the encoder the rest. The calling thread works for both (it runs queued
/// frames while it waits for either), so it is counted once: `decoder + encoder - 1 <=
/// budget`, and two automatic counts never fight over the machine.
pub(crate) fn share_threads(
    budget: usize,
    decoder_threaded: bool,
    encoder_threaded: bool,
) -> (usize, usize) {
    let budget = budget.max(1);
    match (decoder_threaded, encoder_threaded) {
        (true, true) => {
            // Two lets the decoder's MD5 leave the calling thread, which both codecs need.
            let decoder = (budget / 4).max(2).min(budget - 1).max(1);
            (decoder, budget - decoder + 1)
        }
        (true, false) => (budget, 1),
        (false, true) => (1, budget),
        (false, false) => (1, 1),
    }
}

/// The encoder options for `codec`; a level only means something to FLAC.
pub fn encode_options(codec: CodecId, level: Option<u8>) -> Result<EncodeOptions, Error> {
    match level {
        None => Ok(EncodeOptions::default()),
        Some(_) if codec != CodecId::Flac => {
            Err(unsupported("--compression-level applies to flac only"))
        }
        Some(l) if l > 8 => Err(unsupported(format!(
            "compression level {l} (flac takes 0 to 8)"
        ))),
        Some(l) => Ok(EncodeOptions {
            compression_level: Some(l),
            ..EncodeOptions::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SampleFormat::*;

    /// The budget goes whole to the one threaded codec. When both are (FLAC to FLAC) the
    /// decoder, far the cheaper, gets a quarter but at least two (one thread more than the
    /// caller, so that its MD5 leaves the calling thread: on one, the decoder alone held a
    /// re-encode at 52 ms against 22 ms with two), and the encoder the rest; the calling
    /// thread works for both, so together they take no more threads than the budget.
    #[test]
    fn a_budget_is_shared_between_a_threaded_decoder_and_encoder() {
        assert_eq!(share_threads(7, false, true), (1, 7));
        assert_eq!(share_threads(7, true, false), (7, 1));
        assert_eq!(share_threads(7, false, false), (1, 1));
        assert_eq!(share_threads(1, true, true), (1, 1));
        assert_eq!(share_threads(2, true, true), (1, 2));
        assert_eq!(share_threads(3, true, true), (2, 2));
        assert_eq!(share_threads(7, true, true), (2, 6));
        assert_eq!(share_threads(8, true, true), (2, 7));
        assert_eq!(share_threads(64, true, true), (16, 49));
        for budget in 1..40 {
            let (d, e) = share_threads(budget, true, true);
            assert!(d >= 1 && e >= 1 && d + e - 1 <= budget, "{budget}");
        }
    }

    const ALL: [SampleFormat; 6] = [U8, S16, S24, S32, F32, F64];

    fn what(r: Result<impl Sized, Error>) -> String {
        match r {
            Err(Error::Unsupported { what }) => what,
            Err(e) => panic!("wrong error {e:?}"),
            Ok(_) => panic!("expected Unsupported"),
        }
    }

    #[test]
    fn container_defaults() {
        for f in ALL {
            assert_eq!(
                output_codec(FormatId::Wav, f, None),
                Ok(CodecId::pcm_for(f))
            );
            assert_eq!(
                output_codec(FormatId::Aiff, f, None),
                Ok(CodecId::pcm_be_for(f))
            );
        }
        assert_eq!(output_codec(FormatId::Aiff, U8, None), Ok(CodecId::PcmS8));
        for f in [U8, S16, S24, S32] {
            assert_eq!(output_codec(FormatId::Flac, f, None), Ok(CodecId::Flac));
        }
    }

    #[test]
    fn flac_refuses_float() {
        for f in [F32, F64] {
            assert_eq!(
                what(output_codec(FormatId::Flac, f, None)),
                "flac cannot store floating-point samples; choose --sample-format s16, s24 or s32"
            );
        }
    }

    #[test]
    fn ogg_cannot_be_written_yet() {
        assert!(what(output_codec(FormatId::Ogg, S16, None)).contains("ogg"));
    }

    #[test]
    fn requested_codec_must_fit_the_container() {
        assert_eq!(
            output_codec(FormatId::Wav, S16, Some("pcm")),
            Ok(CodecId::PcmS16Le)
        );
        assert_eq!(
            output_codec(FormatId::Aiff, S16, Some("pcm")),
            Ok(CodecId::PcmS16Be)
        );
        assert_eq!(
            output_codec(FormatId::Flac, S16, Some("flac")),
            Ok(CodecId::Flac)
        );
        assert_eq!(
            what(output_codec(FormatId::Wav, S16, Some("flac"))),
            "codec flac cannot be written into wav"
        );
        assert_eq!(
            what(output_codec(FormatId::Flac, S16, Some("pcm"))),
            "codec pcm cannot be written into flac"
        );
        assert_eq!(
            what(output_codec(FormatId::Aiff, S16, Some("mp3"))),
            "codec mp3 cannot be written into aiff"
        );
    }

    #[test]
    fn can_hold_follows_the_container() {
        use CodecId::*;
        let le = [PcmU8, PcmS16Le, PcmS24Le, PcmS32Le, PcmF32Le, PcmF64Le];
        let be = [PcmS8, PcmS16Be, PcmS24Be, PcmS32Be, PcmF32Be, PcmF64Be];
        for c in le {
            assert!(can_hold(FormatId::Wav, c), "{c:?}");
            assert!(!can_hold(FormatId::Aiff, c), "{c:?}");
            assert!(!can_hold(FormatId::Flac, c), "{c:?}");
        }
        for c in be {
            assert!(can_hold(FormatId::Aiff, c), "{c:?}");
            assert!(!can_hold(FormatId::Wav, c), "{c:?}");
        }
        assert!(can_hold(FormatId::Flac, Flac));
        assert!(!can_hold(FormatId::Wav, Flac));
        assert!(!can_hold(FormatId::Aiff, Flac));
        for c in le.into_iter().chain(be).chain([Flac]) {
            assert!(!can_hold(FormatId::Ogg, c), "{c:?}");
        }
    }

    #[test]
    fn compression_level_applies_to_flac_up_to_8() {
        assert_eq!(
            encode_options(CodecId::Flac, None),
            Ok(EncodeOptions::default())
        );
        for l in 0..=8 {
            assert_eq!(
                encode_options(CodecId::Flac, Some(l)),
                Ok(EncodeOptions {
                    compression_level: Some(l),
                    ..EncodeOptions::default()
                })
            );
        }
        assert!(what(encode_options(CodecId::Flac, Some(9))).contains("0 to 8"));
        assert_eq!(
            what(encode_options(CodecId::PcmS16Le, Some(5))),
            "--compression-level applies to flac only"
        );
        assert_eq!(
            encode_options(CodecId::PcmS16Le, None),
            Ok(EncodeOptions::default())
        );
    }
}
