use super::*;

#[test]
fn a_decoder_has_one_or_two_channels() {
    assert!(matches!(Decoder::new(0), Err(Error::BadArgument(_))));
    assert!(matches!(Decoder::new(3), Err(Error::BadArgument(_))));
    for ch in [1, 2] {
        let d = Decoder::new(ch).unwrap();
        assert_eq!(d.channels(), ch);
        assert_eq!(d.final_range(), 0);
    }
}

#[test]
fn an_invalid_packet_is_an_error_and_appends_nothing() {
    let mut d = Decoder::new(2).unwrap();
    let mut out = vec![1.0, 2.0];
    assert!(matches!(
        d.decode(Some(&[]), &mut out),
        Err(Error::InvalidPacket(_))
    ));
    assert!(matches!(
        d.decode(Some(&[(1 << 3) | 1, 0]), &mut out),
        Err(Error::InvalidPacket(_))
    ));
    assert_eq!(out, vec![1.0, 2.0]);
}

#[test]
fn each_mode_goes_to_its_layer() {
    // A 20 ms CELT FB frame, a 20 ms SILK WB frame, a 20 ms hybrid FB frame.
    let celt = [(31 << 3), 0x12, 0x34, 0x56];
    let silk = [(9 << 3), 0x12, 0x34, 0x56];
    let hybrid = [(15 << 3), 0x12, 0x34, 0x56];
    let mut out = vec![];
    let mut d = Decoder::new(2).unwrap();
    assert_eq!(
        d.decode(Some(&celt), &mut out),
        Err(Error::Unsupported("celt decoding is not built yet"))
    );
    // SILK decodes any bytes; hybrid gets past SILK to the part not built.
    assert_eq!(d.decode(Some(&silk), &mut out), Ok(960));
    assert_eq!(out.len(), 960 * 2);
    out.clear();
    assert_eq!(
        d.decode(Some(&hybrid), &mut out),
        Err(Error::Unsupported("hybrid decoding is not built yet"))
    );
    assert!(out.is_empty());
}

#[test]
fn a_lost_packet_before_any_other_is_nothing() {
    let mut d = Decoder::new(1).unwrap();
    let mut out = vec![];
    assert_eq!(d.decode(None, &mut out), Ok(0));
    assert!(out.is_empty());
}

#[test]
fn an_empty_frame_before_any_audio_is_silence_of_its_duration() {
    // opus_decode_frame: a frame of 0 or 1 bytes is concealed; with no mode yet, as zeros.
    // Its final range is 0.
    for (packet, frames) in [
        (&[(31 << 3)][..], 960),
        (&[(16 << 3) | 0b111, 0], 0),
        (&[(28 << 3) | 0b100, 9], 120),
        (&[(1 << 3) | 2, 0], 1920),
    ] {
        let mut d = Decoder::new(2).unwrap();
        let mut out = vec![];
        let r = d.decode(Some(packet), &mut out);
        if frames == 0 {
            assert!(r.is_err(), "{packet:?}");
            continue;
        }
        assert_eq!(r, Ok(frames), "{packet:?}");
        assert_eq!(out, vec![0.0; frames * 2]);
        assert_eq!(d.final_range(), 0);
    }
}

#[test]
fn reset_forgets_the_stream() {
    let mut d = Decoder::new(2).unwrap();
    let mut out = vec![];
    d.decode(Some(&[31 << 3]), &mut out).unwrap();
    d.reset();
    assert_eq!(d.final_range(), 0);
    out.clear();
    assert_eq!(d.decode(None, &mut out), Ok(0));
}
