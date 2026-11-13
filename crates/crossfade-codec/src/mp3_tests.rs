//! What the engine sees of the MP3 decoder: frames, pts, the pre-roll rule and errors. The
//! decoding itself is tested in `crossfade-mp3` and through real files in `crossfade-format`.

use super::*;
use crate::decoder_for;
use crossfade_core::{AudioSpec, ChannelLayout, CodecId, TimeBase};

fn stream(rate: u32, layout: ChannelLayout) -> StreamInfo {
    let audio = AudioSpec::new(rate, SampleFormat::F32, layout).unwrap();
    StreamInfo::new(
        0,
        CodecId::Mp3,
        audio,
        TimeBase::new(1, i64::from(rate)).unwrap(),
    )
}

/// An MPEG-1 128 kbit/s 44.1 kHz frame, joint stereo or mono, with `main_data_begin`.
fn frame(mono: bool, begin: u16) -> Packet {
    let mut data = vec![0u8; 417];
    data[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, if mono { 0xC4 } else { 0x74 }]);
    data[4] = (begin >> 1) as u8;
    data[5] = ((begin & 1) << 7) as u8;
    Packet {
        data,
        ..Packet::default()
    }
}

fn decode(d: &mut dyn Decoder, p: &Packet) -> Result<AudioFrame, Error> {
    d.send_packet(p)?;
    let mut f =
        AudioFrame::new(&AudioSpec::new(8000, SampleFormat::F32, ChannelLayout::mono()).unwrap());
    assert!(d.receive_frame(&mut f)?);
    assert!(!d.receive_frame(&mut f)?);
    Ok(f)
}

#[test]
fn each_packet_gives_one_frame_with_the_packets_pts() {
    let mut d = decoder_for(&stream(44100, ChannelLayout::stereo())).unwrap();
    for (i, pts) in [-529i64, 623, 1775].into_iter().enumerate() {
        let mut p = frame(false, 0);
        p.pts = pts;
        let f = decode(&mut *d, &p).unwrap();
        assert_eq!(
            (f.pts, f.frames(), f.sample_rate),
            (pts, 1152, 44100),
            "{i}"
        );
        assert_eq!(f.layout, ChannelLayout::stereo());
        assert!(matches!(&f.samples, Samples::F32(s) if s.len() == 2304));
    }
    d.drain();
    let mut f =
        AudioFrame::new(&AudioSpec::new(8000, SampleFormat::F32, ChannelLayout::mono()).unwrap());
    assert!(!d.receive_frame(&mut f).unwrap());
}

#[test]
fn a_missing_reservoir_is_silence_until_a_frame_decodes_then_an_error() {
    let mut d = decoder_for(&stream(44100, ChannelLayout::mono())).unwrap();
    let f = decode(&mut *d, &frame(true, 400)).unwrap();
    assert!(matches!(&f.samples, Samples::F32(s) if s.iter().all(|&x| x == 0.0)));
    decode(&mut *d, &frame(true, 0)).unwrap();
    // 396 bytes held; 500 is more.
    d.reset();
    decode(&mut *d, &frame(true, 0)).unwrap();
    let e = decode(&mut *d, &frame(true, 500)).unwrap_err();
    assert_eq!(
        e,
        Error::Malformed {
            what: "mp3: the frame needs 500 bytes of earlier main data, 396 are held".into()
        }
    );
    // After a reset (a seek) it is pre-roll again.
    d.reset();
    assert!(decode(&mut *d, &frame(true, 500)).is_ok());
}

#[test]
fn damaged_frames_and_other_layers_are_errors() {
    let mut d = decoder_for(&stream(44100, ChannelLayout::mono())).unwrap();
    let mut bad = frame(true, 0);
    bad.data.pop();
    assert_eq!(
        decode(&mut *d, &bad).unwrap_err(),
        Error::Malformed {
            what: "mp3: the frame is 416 bytes, its header says 417".into()
        }
    );
    let l2 = Packet {
        data: [&[0xFF, 0xFD, 0x84, 0xC0][..], &[0; 380]].concat(),
        ..Packet::default()
    };
    assert_eq!(
        decode(&mut *d, &l2).unwrap_err(),
        Error::Unsupported {
            what: "mp3: MPEG audio layer II is not supported".into()
        }
    );
}

#[test]
fn a_frame_unlike_the_stream_is_refused() {
    let mut d = decoder_for(&stream(44100, ChannelLayout::mono())).unwrap();
    let e = decode(&mut *d, &frame(false, 0)).unwrap_err();
    assert_eq!(
        e,
        Error::Malformed {
            what: "mp3: a frame of 2 channels at 44100 Hz in a stream of 1 at 44100 Hz".into()
        }
    );
    let mut d = decoder_for(&stream(48000, ChannelLayout::mono())).unwrap();
    assert!(decode(&mut *d, &frame(true, 0)).is_err());
}

#[test]
fn only_f32_mono_or_stereo_streams() {
    let s16 = StreamInfo::new(
        0,
        CodecId::Mp3,
        AudioSpec::new(44100, SampleFormat::S16, ChannelLayout::stereo()).unwrap(),
        TimeBase::new(1, 44100).unwrap(),
    );
    assert!(matches!(
        decoder_for(&s16).err(),
        Some(Error::Unsupported { .. })
    ));
    let six = stream(44100, ChannelLayout::unpositioned(6).unwrap());
    assert!(matches!(
        decoder_for(&six).err(),
        Some(Error::Unsupported { .. })
    ));
}
