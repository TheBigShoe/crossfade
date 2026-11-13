use super::*;
use crate::decoder_for_threads;
use crossfade_core::{AudioSpec, ChannelLayout, CodecId, Packet, SampleFormat, Samples, TimeBase};
use crossfade_opus::testing::ogg as w;
use crossfade_opus::{MultistreamDecoder, OpusHead};

fn stream(head: &[u8], layout: ChannelLayout) -> StreamInfo {
    let audio = AudioSpec::new(48_000, SampleFormat::F32, layout).unwrap();
    let mut s = StreamInfo::new(0, CodecId::Opus, audio, TimeBase::new(1, 48_000).unwrap());
    s.extradata = head.to_vec();
    s
}

/// Every frame the adapter gives for `packets` (pts from -312, as the demuxer stamps them).
fn run(d: &mut dyn Decoder, packets: &[Vec<u8>]) -> Vec<(i64, Vec<f32>)> {
    let mut frames = vec![];
    let mut pts = -312;
    let mut f =
        AudioFrame::new(&AudioSpec::new(48_000, SampleFormat::F32, ChannelLayout::mono()).unwrap());
    for p in packets {
        let duration = crossfade_opus::packet_duration(p).unwrap_or(0) as u64;
        d.send_packet(&Packet {
            stream: 0,
            pts,
            duration,
            data: p.clone(),
        })
        .unwrap();
        while d.receive_frame(&mut f).unwrap() {
            let Samples::F32(s) = &f.samples else {
                panic!("opus decodes to f32")
            };
            frames.push((f.pts, s.clone()));
        }
        pts += duration as i64;
    }
    frames
}

/// The crate's own decoder on the same packets, with the header's gain.
fn direct(head: &[u8], packets: &[Vec<u8>]) -> Vec<Vec<f32>> {
    let h = OpusHead::parse(head).unwrap();
    let mut ms = MultistreamDecoder::new(&h.mapping, usize::from(h.channels)).unwrap();
    ms.set_gain(h.output_gain);
    packets
        .iter()
        .map(|p| {
            let mut out = vec![];
            ms.decode(Some(p), &mut out).unwrap();
            out
        })
        .collect()
}

fn mixed(stereo: bool) -> Vec<Vec<u8>> {
    let mut a = w::packets(9, stereo, 3, 40, 1);
    a.extend(w::packets(15, stereo, 3, 60, 2));
    a.extend(w::packets(31, stereo, 3, 80, 3));
    a
}

#[test]
fn packets_decode_to_frames_stamped_with_their_pts_and_the_headers_gain() {
    for gain in [0, -1536, 700] {
        let head = w::opus_head(2, 312, gain, None);
        let a = mixed(true);
        let mut d = decoder(&stream(&head, ChannelLayout::stereo())).unwrap();
        let got = run(&mut *d, &a);
        let want = direct(&head, &a);
        assert_eq!(got.len(), a.len());
        let mut pts = -312;
        for ((p, s), w) in got.iter().zip(&want) {
            assert_eq!(*p, pts);
            assert_eq!(s, w, "gain {gain}");
            pts += (s.len() / 2) as i64;
        }
    }
}

#[test]
fn family_1_channels_come_out_in_the_layouts_order() {
    // 5.1 in Vorbis order (L C R BL BR LFE): two coupled streams (L/R, BL/BR), two mono.
    let table = [0, 4, 1, 2, 3, 5];
    let head = w::opus_head(6, 0, 0, Some((1, 4, 2, &table)));
    let streams = [
        w::packets(31, true, 2, 60, 4),
        w::packets(31, true, 2, 60, 5),
        w::packets(31, false, 2, 60, 6),
        w::packets(31, false, 2, 60, 7),
    ];
    let a: Vec<Vec<u8>> = (0..2)
        .map(|i| {
            w::multistream_packet(&[
                streams[0][i].clone(),
                streams[1][i].clone(),
                streams[2][i].clone(),
                streams[3][i].clone(),
            ])
        })
        .collect();
    let layout = ChannelLayout::vorbis_order(6).unwrap();
    let mut d = decoder(&stream(&head, layout.clone())).unwrap();
    let got = run(&mut *d, &a);
    let want = direct(&head, &a);
    // Vorbis order: L C R BL BR LFE; the layout stores them in WAVE order.
    use crossfade_core::Channel::*;
    let vorbis = [FrontLeft, FrontCenter, FrontRight, BackLeft, BackRight, Lfe];
    let positions = layout.positions().unwrap();
    for ((_, s), w) in got.iter().zip(&want) {
        for (k, pos) in positions.iter().enumerate() {
            let i = vorbis.iter().position(|v| v == pos).unwrap();
            let ours: Vec<f32> = s.iter().skip(k).step_by(6).copied().collect();
            let theirs: Vec<f32> = w.iter().skip(i).step_by(6).copied().collect();
            assert_eq!(ours, theirs, "{pos:?}");
        }
    }
}

#[test]
fn a_corrupt_packet_is_concealed_for_its_duration_and_one_without_a_toc_skipped() {
    let head = w::opus_head(1, 0, 0, None);
    let mut a = w::packets(9, false, 2, 40, 8);
    // A code 1 packet with an odd payload: invalid, 2 x 20 ms by its TOC.
    a.push(vec![(9 << 3) | 1, 1, 2, 3]);
    a.push(vec![]);
    a.extend(w::packets(9, false, 1, 40, 9));
    let mut d = decoder(&stream(&head, ChannelLayout::mono())).unwrap();
    let got = run(&mut *d, &a);
    assert_eq!(
        got.iter().map(|(p, s)| (*p, s.len())).collect::<Vec<_>>(),
        [(-312, 960), (648, 960), (1608, 1920), (3528, 960)]
    );
    // The concealment is the decoder's.
    let mut ms = crossfade_opus::Decoder::new(1).unwrap();
    let mut out = vec![];
    for p in &a[..2] {
        ms.decode(Some(p), &mut out).unwrap();
    }
    out.clear();
    ms.conceal(1920, &mut out).unwrap();
    assert_eq!(got[2].1, out);
}

#[test]
fn reset_forgets_the_stream() {
    let head = w::opus_head(2, 0, 0, None);
    let a = mixed(false);
    let mut d = decoder(&stream(&head, ChannelLayout::stereo())).unwrap();
    run(&mut *d, &a);
    d.reset();
    let again = run(&mut *d, &a);
    let mut fresh = decoder(&stream(&head, ChannelLayout::stereo())).unwrap();
    assert_eq!(again, run(&mut *fresh, &a));
}

#[test]
fn bad_descriptions_and_headers_are_refused() {
    let head = w::opus_head(2, 0, 0, None);
    let mut s = stream(&head, ChannelLayout::stereo());
    s.audio = AudioSpec::new(44_100, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    assert!(matches!(decoder(&s).err(), Some(Error::Malformed { .. })));
    let s = stream(&head, ChannelLayout::mono());
    assert!(matches!(decoder(&s).err(), Some(Error::Malformed { .. })));
    let s = stream(&head[..12], ChannelLayout::stereo());
    assert!(matches!(decoder(&s).err(), Some(Error::Malformed { .. })));
    let s = stream(
        &w::opus_head(1, 0, 0, Some((2, 1, 0, &[0]))),
        ChannelLayout::unpositioned(1).unwrap(),
    );
    assert_eq!(
        decoder(&s).err(),
        Some(Error::Unsupported {
            what: "opus: ambisonic channel mapping (families 2 and 3) is not supported".into()
        })
    );
    // Through the factory, at any thread count.
    let s = stream(&head, ChannelLayout::stereo());
    assert!(decoder_for_threads(&s, 4).is_ok());
}
