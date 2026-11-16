//! The engine's Opus encoder: 48 kHz float frames of any size in, 20 ms packets out on a
//! timeline that starts `pre_skip` early, and the header that says so.

use crossfade_codec::{EncodeOptions, decoder_for, encoder_for};
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Error, Packet, SampleFormat, Samples,
    StreamInfo, TimeBase,
};
use crossfade_opus::OpusHead;

fn spec(rate: u32, format: SampleFormat, layout: ChannelLayout) -> AudioSpec {
    AudioSpec::new(rate, format, layout).unwrap()
}

fn tone(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|i| {
            let t = i as f64 / 48_000.0;
            let v = 0.4 * (std::f64::consts::TAU * 700.0 * t).sin();
            std::iter::repeat_n(v as f32, channels)
        })
        .collect()
}

/// Feeds `pcm` in frames of `chunk` and returns every packet.
fn encode(s: &AudioSpec, pcm: &[f32], chunk: usize, first_pts: i64) -> (Vec<Packet>, Vec<u8>) {
    let ch = usize::from(s.layout().channels());
    let mut enc = encoder_for(CodecId::Opus, s, &EncodeOptions::default()).unwrap();
    let mut packets = vec![];
    let mut p = Packet::default();
    let mut frame = AudioFrame::new(s);
    let mut pts = first_pts;
    for part in pcm.chunks(chunk * ch) {
        frame.pts = pts;
        frame.samples = Samples::F32(part.to_vec());
        pts += (part.len() / ch) as i64;
        enc.send_frame(&frame).unwrap();
        while enc.receive_packet(&mut p).unwrap() {
            packets.push(p.clone());
        }
    }
    enc.drain();
    while enc.receive_packet(&mut p).unwrap() {
        packets.push(p.clone());
    }
    assert!(!enc.receive_packet(&mut p).unwrap());
    (packets, enc.extradata().to_vec())
}

#[test]
fn packets_cover_the_input_on_a_timeline_that_starts_at_minus_the_pre_skip() {
    for (layout, chunk) in [
        (ChannelLayout::mono(), 1000),
        (ChannelLayout::stereo(), 4096),
    ] {
        let s = spec(48_000, SampleFormat::F32, layout);
        let ch = usize::from(s.layout().channels());
        let frames = 48_000 + 77;
        let pcm = tone(frames, ch);
        let (packets, extradata) = encode(&s, &pcm, chunk, 0);
        let head = OpusHead::parse(&extradata).unwrap();
        assert_eq!(usize::from(head.channels), ch);
        let pre_skip = i64::from(head.pre_skip);
        assert!(pre_skip > 0);
        // Enough 20 ms packets for the input and the encoder's delay, and no more.
        assert_eq!(packets.len(), (frames + pre_skip as usize).div_ceil(960));
        for (k, p) in packets.iter().enumerate() {
            assert_eq!(
                (p.pts, p.duration),
                (k as i64 * 960 - pre_skip, 960),
                "packet {k}"
            );
            assert_eq!(crossfade_opus::packet_duration(&p.data).unwrap(), 960);
        }

        // Through the engine's decoder: the input again, `pre_skip` late.
        let mut stream = StreamInfo::new(
            0,
            CodecId::Opus,
            s.clone(),
            TimeBase::new(1, 48_000).unwrap(),
        );
        stream.extradata = extradata;
        let mut dec = decoder_for(&stream).unwrap();
        let mut out = vec![];
        let mut f = AudioFrame::new(&s);
        for p in &packets {
            dec.send_packet(p).unwrap();
            while dec.receive_frame(&mut f).unwrap() {
                let Samples::F32(v) = &f.samples else {
                    panic!("f32")
                };
                out.extend_from_slice(v);
            }
        }
        let got = &out[pre_skip as usize * ch..][..frames * ch];
        let (mut sig, mut err) = (0.0f64, 0.0f64);
        for (w, g) in pcm.iter().zip(got).skip(4000 * ch) {
            sig += f64::from(*w).powi(2);
            err += f64::from(w - g).powi(2);
        }
        assert!(sig / err > 100.0, "{ch} ch: snr {:.1}", sig / err);
    }
}

#[test]
fn the_timeline_follows_the_first_frames_pts_and_nothing_in_gives_nothing_out() {
    let s = spec(48_000, SampleFormat::F32, ChannelLayout::mono());
    let (packets, _) = encode(&s, &tone(960, 1), 960, 5000);
    assert_eq!(packets[0].pts, 5000 - 120);
    let (packets, extradata) = encode(&s, &[], 960, 0);
    assert!(packets.is_empty());
    // The header is known before any audio: a muxer writes it first.
    assert!(OpusHead::parse(&extradata).is_ok());
}

#[test]
fn only_48_khz_float_mono_or_stereo_is_taken() {
    let refused = |s: AudioSpec| match encoder_for(CodecId::Opus, &s, &EncodeOptions::default()) {
        Err(Error::Unsupported { what }) => what,
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("accepted"),
    };
    assert_eq!(
        refused(spec(44_100, SampleFormat::F32, ChannelLayout::stereo())),
        "opus is encoded at 48000 Hz, not 44100 Hz"
    );
    assert_eq!(
        refused(spec(48_000, SampleFormat::S16, ChannelLayout::stereo())),
        "opus is encoded from f32 samples, not s16"
    );
    let surround = ChannelLayout::from_name("5.1").unwrap();
    assert_eq!(
        refused(spec(48_000, SampleFormat::F32, surround)),
        "opus encoding takes mono or stereo, not 6 channels; choose --channels stereo"
    );
}
