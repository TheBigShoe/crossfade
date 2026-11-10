use crossfade_codec::{decoder_for, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Packet, SampleFormat, StreamInfo, TimeBase,
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
        let mut enc = encoder_for(codec, &audio).unwrap();
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
