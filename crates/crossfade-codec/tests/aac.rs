//! The AAC decoder adapter. With the `aac` feature it decodes; without it, an AAC stream is
//! refused with a message saying why.

use crossfade_core::{
    AudioSpec, ChannelLayout, CodecId, Error, SampleFormat, StreamInfo, TimeBase,
};

fn stream(extradata: &[u8], rate: u32, layout: ChannelLayout) -> StreamInfo {
    let audio = AudioSpec::new(rate, SampleFormat::F32, layout).unwrap();
    let mut s = StreamInfo::new(
        0,
        CodecId::Aac,
        audio,
        TimeBase::new(1, i64::from(rate)).unwrap(),
    );
    s.extradata = extradata.to_vec();
    s
}

#[cfg(not(feature = "aac"))]
#[test]
fn without_the_feature_aac_is_refused_and_the_message_says_why() {
    let s = stream(&[0x12, 0x10], 44100, ChannelLayout::stereo());
    assert_eq!(
        crossfade_codec::decoder_for(&s).err(),
        Some(Error::Unsupported {
            what: "aac decoding is not part of this build (AAC is patent-encumbered); \
                   build with --features aac"
                .into()
        })
    );
}

#[cfg(feature = "aac")]
mod with_the_feature {
    use super::*;
    use crossfade_aac::adts::Header;
    use crossfade_core::{AudioFrame, Packet, Samples};

    const FIXTURES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crossfade-format/tests/fixtures/adts"
    );

    fn packets(name: &str) -> Vec<Packet> {
        let bytes = std::fs::read(format!("{FIXTURES}/{name}")).unwrap();
        let mut out = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let h = Header::parse(&bytes[at..]).unwrap();
            let end = at + usize::from(h.frame_length);
            for b in h.raw_data_blocks(&bytes[at..end]).unwrap() {
                out.push(Packet {
                    stream: 0,
                    pts: out.len() as i64 * 1024 - 2112,
                    duration: 1024,
                    data: b.data.to_vec(),
                });
            }
            at = end;
        }
        out
    }

    fn decode_all(d: &mut dyn crossfade_core::Decoder, packets: &[Packet]) -> Vec<AudioFrame> {
        let mut frames = Vec::new();
        for p in packets {
            d.send_packet(p).unwrap();
            let mut f = AudioFrame::new(
                &AudioSpec::new(8000, SampleFormat::F32, ChannelLayout::mono()).unwrap(),
            );
            while d.receive_frame(&mut f).unwrap() {
                frames.push(f.clone());
            }
        }
        d.drain();
        let mut f = frames[0].clone();
        assert!(!d.receive_frame(&mut f).unwrap());
        frames
    }

    #[test]
    fn frames_are_float_in_the_streams_layout_stamped_with_their_packets_pts() {
        let s = stream(&[0x12, 0x10], 44100, ChannelLayout::stereo());
        let mut d = crossfade_codec::decoder_for(&s).unwrap();
        let packets = packets("tone_44k_stereo.aac");
        let frames = decode_all(&mut *d, &packets);
        assert_eq!(frames.len(), packets.len());
        for (f, p) in frames.iter().zip(&packets) {
            assert_eq!(f.pts, p.pts);
            assert_eq!(f.frames(), 1024);
            assert_eq!(f.sample_rate, 44100);
            assert_eq!(f.layout, ChannelLayout::stereo());
            assert!(matches!(f.samples, Samples::F32(_)));
        }
        // A tone at -6 dBFS: well inside -1..1, and not silence.
        let Samples::F32(mid) = &frames[20].samples else {
            unreachable!()
        };
        let peak = mid.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!((0.3..0.7).contains(&peak), "{peak}");
    }

    #[test]
    fn a_reset_decoder_decodes_as_a_new_one() {
        let s = stream(&[0x12, 0x10], 44100, ChannelLayout::stereo());
        let packets = packets("clicks_44k_stereo.aac");
        let mut d = crossfade_codec::decoder_for(&s).unwrap();
        let first = decode_all(&mut *d, &packets[..8]);
        // Mid-stream, then reset: as if new.
        decode_all(&mut *d, &packets[20..23]);
        d.reset();
        let again = decode_all(&mut *d, &packets[..8]);
        assert_eq!(first, again);
    }

    #[test]
    fn a_packet_of_several_blocks_gives_one_frame_as_long_as_them_all() {
        let s = stream(&[0x12, 0x10], 44100, ChannelLayout::stereo());
        let packets = packets("tone_44k_stereo.aac");
        let mut d = crossfade_codec::decoder_for(&s).unwrap();
        let one = decode_all(&mut *d, &packets[..3]);
        d.reset();
        let joined = Packet {
            stream: 0,
            pts: packets[0].pts,
            duration: 3 * 1024,
            data: packets[..3].iter().flat_map(|p| p.data.clone()).collect(),
        };
        let all = decode_all(&mut *d, &[joined]);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].pts, packets[0].pts);
        let concat: Vec<f32> = one
            .iter()
            .flat_map(|f| match &f.samples {
                Samples::F32(v) => v.clone(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(all[0].samples, Samples::F32(concat));
    }

    #[test]
    fn a_stream_the_decoder_cannot_take_is_refused_by_name() {
        // AAC Main.
        let s = stream(&[0x0A, 0x10], 44100, ChannelLayout::stereo());
        assert_eq!(
            crossfade_codec::decoder_for(&s).err(),
            Some(Error::Unsupported {
                what: "aac: AAC Main (prediction) is not supported".into()
            })
        );
        // The container's layout must have the configuration's channel count.
        let s = stream(&[0x12, 0x10], 44100, ChannelLayout::mono());
        assert_eq!(
            crossfade_codec::decoder_for(&s).err(),
            Some(Error::Malformed {
                what: "aac: the stream has 1 channels, its configuration 2".into()
            })
        );
        assert!(matches!(
            crossfade_codec::decoder_for(&stream(&[], 44100, ChannelLayout::stereo())).err(),
            Some(Error::Malformed { .. })
        ));
    }

    #[test]
    fn a_broken_block_is_an_error_and_the_next_decodes() {
        let s = stream(&[0x12, 0x10], 44100, ChannelLayout::stereo());
        let packets = packets("tone_44k_stereo.aac");
        let mut d = crossfade_codec::decoder_for(&s).unwrap();
        let mut bad = packets[5].clone();
        bad.data.truncate(4);
        assert!(matches!(d.send_packet(&bad), Err(Error::Malformed { .. })));
        d.send_packet(&packets[6]).unwrap();
        let mut f = AudioFrame::new(&s.audio);
        assert!(d.receive_frame(&mut f).unwrap());
        assert_eq!(f.pts, packets[6].pts);
    }
}
