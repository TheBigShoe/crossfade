use crossfade_codec::{EncodeOptions, decoder_for, encoder_for, encoder_for_threads};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Error, Packet, SampleFormat, StreamInfo,
    TimeBase,
};

#[test]
fn boxed_codecs_round_trip_for_every_pcm_codec() {
    for format in [
        SampleFormat::U8,
        SampleFormat::S16,
        SampleFormat::S24,
        SampleFormat::S32,
        SampleFormat::F32,
        SampleFormat::F64,
    ] {
        let audio = AudioSpec::new(8000, format, ChannelLayout::stereo()).unwrap();
        let codec = CodecId::pcm_for(format);
        let info = StreamInfo::new(0, codec, audio.clone(), TimeBase::new(1, 8000).unwrap());
        let mut dec = decoder_for(&info).unwrap();
        let mut enc = encoder_for(codec, &audio, &EncodeOptions::default()).unwrap();
        let bytes: Vec<u8> = (0..format.bytes_per_sample() * 6)
            .map(|i| i as u8)
            .collect();
        let pkt = Packet {
            data: bytes.clone(),
            ..Packet::default()
        };
        dec.send_packet(&pkt).unwrap();
        let mut frame = AudioFrame::new(&audio);
        assert!(dec.receive_frame(&mut frame).unwrap());
        enc.send_frame(&frame).unwrap();
        let mut out = Packet::default();
        assert!(enc.receive_packet(&mut out).unwrap());
        // NaN payloads and S24 high bits survive: compare bytes, not values.
        assert_eq!(out.data, bytes, "{format:?}");
    }
}

#[test]
fn flac_is_routed_to_the_real_codecs() {
    let audio = AudioSpec::new(8000, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    let info = StreamInfo::new(
        0,
        CodecId::Flac,
        audio.clone(),
        TimeBase::new(1, 8000).unwrap(),
    );
    // Routed to the FLAC decoder, which finds no STREAMINFO in the empty extradata.
    assert!(matches!(decoder_for(&info), Err(Error::Malformed { .. })));
    let opts = EncodeOptions {
        compression_level: Some(5),
    };
    assert!(encoder_for(CodecId::Flac, &audio, &opts).is_ok());
    // A thread count is accepted by every encoder, and PCM ignores it.
    for threads in [0, 1, 4] {
        assert!(encoder_for_threads(CodecId::Flac, &audio, &opts, threads).is_ok());
        let pcm = CodecId::pcm_for(SampleFormat::S16);
        assert!(encoder_for_threads(pcm, &audio, &EncodeOptions::default(), threads).is_ok());
    }
}
#[test]
fn the_factories_build_big_endian_and_signed_8_bit_pcm() {
    for (codec, format) in [
        (CodecId::PcmS8, SampleFormat::U8),
        (CodecId::PcmS16Be, SampleFormat::S16),
        (CodecId::PcmS24Be, SampleFormat::S24),
        (CodecId::PcmS32Be, SampleFormat::S32),
        (CodecId::PcmF32Be, SampleFormat::F32),
        (CodecId::PcmF64Be, SampleFormat::F64),
    ] {
        let audio = AudioSpec::new(8000, format, ChannelLayout::mono()).unwrap();
        let info = StreamInfo::new(0, codec, audio.clone(), TimeBase::new(1, 8000).unwrap());
        assert!(decoder_for(&info).is_ok(), "{codec:?}");
        assert!(
            encoder_for(codec, &audio, &EncodeOptions::default()).is_ok(),
            "{codec:?}"
        );
    }
}
