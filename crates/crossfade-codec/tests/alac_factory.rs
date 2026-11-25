use crossfade_codec::{
    EncodeOptions, encoder_for, encoder_for_threads, is_threaded, is_threaded_encoder,
};
use crossfade_core::{AudioSpec, ChannelLayout, CodecId, SampleFormat};

#[test]
fn alac_has_an_encoder_whatever_the_thread_count() {
    let audio = AudioSpec::new(44100, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    let opts = EncodeOptions::default();
    assert_eq!(
        encoder_for(CodecId::Alac, &audio, &opts)
            .unwrap()
            .extradata()
            .len(),
        24
    );
    for threads in [0, 1, 4] {
        assert!(encoder_for_threads(CodecId::Alac, &audio, &opts, threads).is_ok());
    }
}

#[test]
fn only_the_flac_encoder_can_use_threads_but_both_flac_and_alac_decoders_can() {
    assert!(is_threaded(CodecId::Alac) && is_threaded(CodecId::Flac));
    assert!(is_threaded_encoder(CodecId::Flac));
    assert!(!is_threaded_encoder(CodecId::Alac));
    assert!(!is_threaded_encoder(CodecId::PcmS16Le));
}

#[test]
fn opus_has_an_encoder_that_uses_no_threads() {
    let audio = AudioSpec::new(48000, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    let opts = EncodeOptions::default();
    assert!(!is_threaded_encoder(CodecId::Opus));
    for threads in [0, 1, 4] {
        let e = encoder_for_threads(CodecId::Opus, &audio, &opts, threads).unwrap();
        assert_eq!(&e.extradata()[..8], b"OpusHead");
    }
}
