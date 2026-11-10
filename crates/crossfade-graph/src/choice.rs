#[cfg(test)]
mod tests {
    use super::*;
    use SampleFormat::*;

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
            assert_eq!(output_codec(FormatId::Wav, f, None), Ok(CodecId::pcm_for(f)));
            assert_eq!(output_codec(FormatId::Aiff, f, None), Ok(CodecId::pcm_be_for(f)));
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
        assert_eq!(output_codec(FormatId::Wav, S16, Some("pcm")), Ok(CodecId::PcmS16Le));
        assert_eq!(output_codec(FormatId::Aiff, S16, Some("pcm")), Ok(CodecId::PcmS16Be));
        assert_eq!(output_codec(FormatId::Flac, S16, Some("flac")), Ok(CodecId::Flac));
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
        assert_eq!(encode_options(CodecId::Flac, None), Ok(EncodeOptions::default()));
        for l in 0..=8 {
            assert_eq!(
                encode_options(CodecId::Flac, Some(l)),
                Ok(EncodeOptions {
                    compression_level: Some(l)
                })
            );
        }
        assert!(what(encode_options(CodecId::Flac, Some(9))).contains("0 to 8"));
        assert_eq!(
            what(encode_options(CodecId::PcmS16Le, Some(5))),
            "--compression-level applies to flac only"
        );
        assert_eq!(encode_options(CodecId::PcmS16Le, None), Ok(EncodeOptions::default()));
    }
}
