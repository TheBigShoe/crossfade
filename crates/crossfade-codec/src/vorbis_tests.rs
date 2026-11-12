use super::*;
use crate::decoder_for_threads;
use crossfade_core::{AudioSpec, Channel, CodecId, Packet, SampleFormat, Samples, TimeBase};
use crossfade_vorbis::writer::{self as w, Book};

/// A stream of `channels` channels whose channel c has the flat floor 100 + 20 c and a
/// residue of ones, so every channel is a different, known signal.
struct Built {
    headers: [Vec<u8>; 3],
    packets: Vec<Vec<u8>>,
}

fn build(channels: u8, modes: &str) -> Built {
    let book = Book {
        dims: 1,
        ..Book::flat(1)
    }
    .with_values(0, &[0, 1]);
    let setup = w::simple_setup(1, book.clone(), 64, 8);
    let words = book.codewords();
    let seq: Vec<u8> = modes.bytes().map(|b| u8::from(b == b'L')).collect();
    let mut packets = vec![];
    for (i, &m) in seq.iter().enumerate() {
        let prev = i.checked_sub(1).is_none_or(|j| seq[j] == 1);
        let next = seq.get(i + 1).is_none_or(|&n| n == 1);
        let mut wr = setup.start_packet(m, prev, next);
        for c in 0..channels {
            wr.put_bit(true);
            wr.put(100 + 20 * u64::from(c), 8);
            wr.put(100 + 20 * u64::from(c), 8);
        }
        for _ in 0..8 {
            for _ in 0..channels {
                wr.put_bit(false);
            }
            for _ in 0..channels {
                for k in 0..8 {
                    Book::encode_with(&words, &mut wr, u32::from((k + i) % 3 == 0));
                }
            }
        }
        packets.push(wr.into_bytes());
    }
    Built {
        headers: [
            w::ident(channels, 48_000, [256, 512]),
            w::comment("test", &[]),
            setup.write(channels),
        ],
        packets,
    }
}

fn stream(b: &Built, layout: ChannelLayout) -> StreamInfo {
    let audio = AudioSpec::new(48_000, SampleFormat::F32, layout).unwrap();
    let mut s = StreamInfo::new(0, CodecId::Vorbis, audio, TimeBase::new(1, 48_000).unwrap());
    s.extradata = crossfade_vorbis::pack_headers([&b.headers[0], &b.headers[1], &b.headers[2]]);
    s
}

/// What the standalone decoder gives for the same packets, in Vorbis channel order.
fn direct(b: &Built) -> Vec<Vec<f32>> {
    let ident = crossfade_vorbis::Ident::parse(&b.headers[0]).unwrap();
    let setup = crossfade_vorbis::Setup::parse(&b.headers[2], &ident, usize::MAX).unwrap();
    let mut d = crossfade_vorbis::Decoder::new(&ident, setup).unwrap();
    b.packets
        .iter()
        .map(|p| {
            let mut out = vec![];
            d.decode(p, &mut out).unwrap();
            out
        })
        .collect()
}

fn packet(data: &[u8], pts: i64) -> Packet {
    Packet {
        stream: 0,
        pts,
        duration: 0,
        data: data.to_vec(),
    }
}

/// Sends each packet and collects what comes out, as (pts, frames, samples).
fn run(d: &mut dyn Decoder, packets: &[Packet]) -> Vec<(i64, usize, Vec<f32>)> {
    let mut out = vec![];
    let mut f =
        AudioFrame::new(&AudioSpec::new(1, SampleFormat::S16, ChannelLayout::mono()).unwrap());
    for p in packets {
        d.send_packet(p).unwrap();
        while d.receive_frame(&mut f).unwrap() {
            let Samples::F32(s) = &f.samples else {
                panic!("{:?}", f.samples.format())
            };
            out.push((f.pts, f.frames(), s.clone()));
        }
    }
    out
}

#[test]
fn frames_carry_their_packets_pts_and_a_packet_that_yields_nothing_yields_no_frame() {
    let b = build(2, "SLLSS");
    let s = stream(&b, ChannelLayout::stereo());
    let mut d = decoder_for_threads(&s, 1).unwrap();
    let pts = [-128i64, -64, 320, 832, 1088];
    let packets: Vec<Packet> = b
        .packets
        .iter()
        .zip(pts)
        .map(|(p, t)| packet(p, t))
        .collect();
    let got = run(&mut *d, &packets);
    let want = direct(&b);
    assert_eq!(got.len(), 4);
    for (k, (pts, frames, samples)) in got.iter().enumerate() {
        assert_eq!(*pts, packets[k + 1].pts);
        assert_eq!(*samples, want[k + 1]);
        assert_eq!(*frames, want[k + 1].len() / 2);
    }
    assert_eq!(
        got.iter().map(|g| g.1).collect::<Vec<_>>(),
        [64 + 128, 256, 192, 128]
    );
    d.drain();
    let mut f = AudioFrame::new(&s.audio);
    assert_eq!(d.receive_frame(&mut f), Ok(false));
}

#[test]
fn channels_are_labelled_from_the_specifications_table_and_stored_in_mask_order() {
    let names = [
        (1, Some("mono")),
        (2, Some("stereo")),
        (3, Some("3.0")),
        (4, Some("quad")),
        (5, Some("5.0-back")),
        (6, Some("5.1-back")),
        (7, Some("6.1")),
        (8, Some("7.1")),
    ];
    for (n, name) in names {
        assert_eq!(layout(n).name(), name, "{n} channels");
        assert_eq!(layout(n).channels(), u16::from(n));
    }
    assert_eq!(layout(9), ChannelLayout::unpositioned(9).unwrap());
    assert_eq!(layout(255).channels(), 255);
}

#[test]
fn six_channels_come_out_in_the_layouts_order() {
    use Channel::*;
    let b = build(6, "SSS");
    let want = direct(&b);
    // Vorbis order: FL C FR RL RR LFE; 5.1-back storage order: FL FR FC LFE BL BR.
    let vorbis_to_storage = [0, 2, 1, 4, 5, 3];
    let s = stream(&b, layout(6));
    assert_eq!(
        layout(6).positions(),
        Some(&[FrontLeft, FrontRight, FrontCenter, Lfe, BackLeft, BackRight][..])
    );
    let mut d = decoder_for_threads(&s, 1).unwrap();
    let packets: Vec<Packet> = b.packets.iter().map(|p| packet(p, 0)).collect();
    let got = run(&mut *d, &packets);
    for (g, w) in got.iter().zip(&want[1..]) {
        for (gf, wf) in g.2.chunks(6).zip(w.chunks(6)) {
            for (c, &slot) in vorbis_to_storage.iter().enumerate() {
                assert_eq!(gf[slot], wf[c]);
            }
        }
    }
    // A stream description that keeps Vorbis's order unlabelled gets it unchanged.
    let s = stream(&b, ChannelLayout::unpositioned(6).unwrap());
    let mut d = decoder_for_threads(&s, 1).unwrap();
    let got = run(&mut *d, &packets);
    assert_eq!(got[0].2, want[1]);
}

#[test]
fn a_bad_packet_yields_nothing_and_the_stream_goes_on() {
    let b = build(1, "SSSS");
    let s = stream(&b, ChannelLayout::mono());
    let mut d = decoder_for_threads(&s, 1).unwrap();
    let mut packets = vec![];
    for (i, p) in b.packets.iter().enumerate() {
        packets.push(packet(p, 128 * i as i64));
        packets.push(packet(&[0x05, b'v', b'o'], 0));
        packets.push(packet(&[], 0));
    }
    let got = run(&mut *d, &packets);
    let want = direct(&b);
    assert_eq!(got.len(), 3);
    for (g, w) in got.iter().zip(&want[1..]) {
        assert_eq!(&g.2, w);
    }
}

#[test]
fn reset_forgets_the_overlap() {
    let b = build(2, "SLLS");
    let s = stream(&b, ChannelLayout::stereo());
    let packets: Vec<Packet> = b.packets.iter().map(|p| packet(p, 7)).collect();
    let mut fresh = decoder_for_threads(&s, 1).unwrap();
    let want = run(&mut *fresh, &packets[2..]);
    let mut d = decoder_for_threads(&s, 1).unwrap();
    run(&mut *d, &packets[..2]);
    d.reset();
    assert_eq!(run(&mut *d, &packets[2..]), want);
}

#[test]
fn a_bad_header_blob_or_description_is_refused() {
    let b = build(2, "S");
    let malformed = |s: &StreamInfo| match decoder_for_threads(s, 1).err() {
        Some(Error::Malformed { what }) => what,
        other => panic!("{other:?}"),
    };
    let mut s = stream(&b, ChannelLayout::stereo());
    s.extradata.clear();
    assert!(malformed(&s).contains("vorbis"));
    let mut s = stream(&b, ChannelLayout::stereo());
    s.extradata.truncate(s.extradata.len() - 1);
    assert!(malformed(&s).contains("setup"));
    let s = stream(&b, ChannelLayout::mono());
    assert!(malformed(&s).contains("disagrees"));
    let mut s = stream(&b, ChannelLayout::stereo());
    s.audio = AudioSpec::new(44_100, SampleFormat::F32, ChannelLayout::stereo()).unwrap();
    assert!(malformed(&s).contains("disagrees"));
    let mut s = stream(&b, ChannelLayout::stereo());
    s.audio = AudioSpec::new(48_000, SampleFormat::S16, ChannelLayout::stereo()).unwrap();
    assert!(malformed(&s).contains("disagrees"));
    let s = stream(
        &b,
        ChannelLayout::from_positions(vec![Channel::FrontLeft, Channel::Lfe]).unwrap(),
    );
    assert!(malformed(&s).contains("layout"));
}
