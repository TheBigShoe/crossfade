use super::*;
use crate::bits::BitWriter;
use crate::flac::crc::crc16;
use crate::flac::frame::{self, ChannelAssignment, FrameHeader};
use crate::flac::streaminfo::StreamInfoBlock;
use crate::md5::Md5;
use crossfade_core::{
    AudioFrame, AudioSpec, ChannelLayout, CodecId, Packet, SampleFormat, Samples, TimeBase,
};
use proptest::prelude::*;

// ---- building blocks: streams, frames and subframes assembled bit by bit ----

fn block_info(rate: u32, channels: u8, bits: u8, total: u64, md5: [u8; 16]) -> StreamInfoBlock {
    StreamInfoBlock {
        min_block_size: 16,
        max_block_size: 4096,
        min_frame_size: 0,
        max_frame_size: 0,
        sample_rate: rate,
        channels,
        bits_per_sample: bits,
        total_samples: total,
        md5,
    }
}

fn stream_for(b: &StreamInfoBlock) -> StreamInfo {
    let format = match b.bits_per_sample {
        0..=8 => SampleFormat::U8,
        9..=16 => SampleFormat::S16,
        17..=24 => SampleFormat::S24,
        _ => SampleFormat::S32,
    };
    let layout = if b.channels == 2 {
        ChannelLayout::stereo()
    } else {
        ChannelLayout::unpositioned(u16::from(b.channels)).unwrap()
    };
    StreamInfo {
        extradata: b.write().to_vec(),
        ..StreamInfo::new(
            0,
            CodecId::Flac,
            AudioSpec::new(b.sample_rate, format, layout).unwrap(),
            TimeBase::new(1, i64::from(b.sample_rate)).unwrap(),
        )
    }
}

fn stream(rate: u32, channels: u8, bits: u8) -> StreamInfo {
    stream_for(&block_info(rate, channels, bits, 0, [0; 16]))
}

/// A frame around `body`: header with its CRC-8 and the CRC-16 trailer.
fn frame_around(
    block: u32,
    channels: ChannelAssignment,
    bits: Option<u8>,
    rate: Option<u32>,
    body: Vec<u8>,
) -> Vec<u8> {
    let h = FrameHeader {
        variable_block_size: false,
        block_size: block,
        sample_rate: rate,
        channels,
        bits_per_sample: bits,
        number: 0,
        header_len: 0,
    };
    let mut f = Vec::new();
    frame::write_header(&h, &mut f);
    f.extend(body);
    let crc = crc16(&f);
    f.extend_from_slice(&crc.to_be_bytes());
    f
}

fn body(build: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut w = BitWriter::new();
    build(&mut w);
    w.byte_align();
    w.into_bytes()
}

fn decode(info: &StreamInfo, frame: &[u8]) -> Result<AudioFrame, Error> {
    let mut d = decoder(info)?;
    d.send_packet(&Packet {
        stream: 0,
        pts: 77,
        duration: 0,
        data: frame.to_vec(),
    })?;
    let mut out = AudioFrame::new(&info.audio);
    assert!(d.receive_frame(&mut out)?);
    assert_eq!(out.pts, 77);
    Ok(out)
}

fn s16(f: &AudioFrame) -> Vec<i16> {
    match &f.samples {
        Samples::S16(v) => v.clone(),
        other => panic!("not S16: {other:?}"),
    }
}

fn s32(f: &AudioFrame) -> Vec<i32> {
    match &f.samples {
        Samples::S32(v) => v.clone(),
        other => panic!("not S32: {other:?}"),
    }
}

fn unsupported(r: Result<AudioFrame, Error>) -> String {
    match r {
        Err(Error::Unsupported { what }) => what,
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

fn malformed(r: Result<AudioFrame, Error>) -> String {
    match r {
        Err(Error::Malformed { what }) => what,
        Err(Error::UnexpectedEof { .. }) => "eof".into(),
        other => panic!("expected Malformed, got {other:?}"),
    }
}

/// Subframe header: pad bit, 6-bit type, wasted-bits flag (and its unary count).
fn sub_header(w: &mut BitWriter, kind: u64, wasted: u32) {
    w.write_bits(0, 1);
    w.write_bits(kind, 6);
    if wasted == 0 {
        w.write_bits(0, 1);
    } else {
        w.write_bits(1, 1);
        w.write_unary(wasted - 1);
    }
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn write_rice(w: &mut BitWriter, v: i64, k: u32) {
    let u = zigzag(v);
    w.write_unary((u >> k) as u32);
    w.write_bits(u & ((1u64 << k) - 1), k);
}

/// How one residual partition is coded.
#[derive(Clone, Copy)]
enum Part {
    Rice(u32),
    Escape(u32),
}

/// Residual section: method bit, partition order, then the partitions over `res`, which holds
/// the block minus the predictor order.
fn write_residual(
    w: &mut BitWriter,
    five_bit: bool,
    order: u32,
    block: usize,
    pred: usize,
    res: &[i64],
    parts: &[Part],
) {
    w.write_bits(u64::from(five_bit), 2);
    w.write_bits(u64::from(order), 4);
    let n = 1usize << order;
    assert_eq!(parts.len(), n);
    let (pbits, escape) = if five_bit { (5, 31) } else { (4, 15) };
    let mut at = 0;
    for (p, part) in parts.iter().enumerate() {
        let count = (block >> order) - if p == 0 { pred } else { 0 };
        let vals = &res[at..at + count];
        at += count;
        match *part {
            Part::Rice(k) => {
                w.write_bits(u64::from(k), pbits);
                for &v in vals {
                    write_rice(w, v, k);
                }
            }
            Part::Escape(bits) => {
                w.write_bits(escape, pbits);
                w.write_bits(u64::from(bits), 5);
                for &v in vals {
                    w.write_signed(v, bits);
                }
            }
        }
    }
    assert_eq!(at, res.len());
}

fn rice_parts(order: u32) -> Vec<Part> {
    vec![Part::Rice(3); 1 << order]
}

fn fixed_prediction(order: usize, x: &[i64]) -> i64 {
    let at = |k: usize| x[x.len() - k];
    match order {
        0 => 0,
        1 => at(1),
        2 => 2 * at(1) - at(2),
        3 => 3 * at(1) - 3 * at(2) + at(3),
        _ => 4 * at(1) - 6 * at(2) + 4 * at(3) - at(4),
    }
}

fn sub_fixed(w: &mut BitWriter, order: usize, x: &[i64], bps: u32, wasted: u32) {
    sub_header(w, 8 + order as u64, wasted);
    let x: Vec<i64> = x.iter().map(|v| v >> wasted).collect();
    for &v in &x[..order] {
        w.write_signed(v, bps - wasted);
    }
    let res: Vec<i64> = (order..x.len())
        .map(|i| x[i] - fixed_prediction(order, &x[..i]))
        .collect();
    write_residual(w, false, 0, x.len(), order, &res, &rice_parts(0));
}

fn lpc_prediction(coefs: &[i64], shift: u32, x: &[i64]) -> i64 {
    let sum: i64 = coefs
        .iter()
        .enumerate()
        .map(|(j, c)| c * x[x.len() - 1 - j])
        .sum();
    sum >> shift
}

fn sub_lpc(w: &mut BitWriter, coefs: &[i64], precision: u32, shift: u32, x: &[i64], bps: u32) {
    let order = coefs.len();
    sub_header(w, 31 + order as u64, 0);
    for &v in &x[..order] {
        w.write_signed(v, bps);
    }
    w.write_bits(u64::from(precision - 1), 4);
    w.write_bits(u64::from(shift), 5);
    for &c in coefs {
        w.write_signed(c, precision);
    }
    let res: Vec<i64> = (order..x.len())
        .map(|i| x[i] - lpc_prediction(coefs, shift, &x[..i]))
        .collect();
    write_residual(w, false, 0, x.len(), order, &res, &rice_parts(0));
}

fn sub_verbatim(w: &mut BitWriter, x: &[i64], bps: u32) {
    sub_header(w, 1, 0);
    for &v in x {
        w.write_signed(v, bps);
    }
}

fn sub_constant(w: &mut BitWriter, v: i64, bps: u32) {
    sub_header(w, 0, 0);
    w.write_signed(v, bps);
}

/// A wandering signal of `n` samples within `bits`, deterministic.
fn wave(n: usize, bits: u32, seed: i64) -> Vec<i64> {
    let max = (1i64 << (bits - 1)) - 1;
    let mut s = seed;
    (0..n)
        .map(|i| {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407 + i as i64)
                .rem_euclid(1 << 40);
            let v = (s >> 8) % (max / 4 + 1);
            (v * if i % 3 == 0 { -1 } else { 1 } + (i as i64 * (max / 200 + 1)) % (max / 2 + 1))
                .clamp(-max - 1, max)
        })
        .collect()
}

fn mono_frame(x: &[i64], bps: u8, build: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    frame_around(
        x.len() as u32,
        ChannelAssignment::Independent(1),
        Some(bps),
        None,
        body(build),
    )
}

// ---- subframe types ----

#[test]
fn a_constant_subframe_fills_the_block() {
    let f = mono_frame(&[0; 32], 16, |w| sub_constant(w, -1234, 16));
    let out = decode(&stream(44100, 1, 16), &f).unwrap();
    assert_eq!(s16(&out), vec![-1234i16; 32]);
}

#[test]
fn a_verbatim_subframe_is_copied() {
    let x = wave(50, 16, 1);
    let f = mono_frame(&x, 16, |w| sub_verbatim(w, &x, 16));
    let out = decode(&stream(44100, 1, 16), &f).unwrap();
    assert_eq!(s16(&out), x.iter().map(|&v| v as i16).collect::<Vec<_>>());
}

#[test]
fn every_fixed_predictor_order_restores_the_signal() {
    let x = wave(100, 16, 2);
    for order in 0..=4 {
        let f = mono_frame(&x, 16, |w| sub_fixed(w, order, &x, 16, 0));
        let out = decode(&stream(44100, 1, 16), &f).unwrap();
        assert_eq!(
            s16(&out),
            x.iter().map(|&v| v as i16).collect::<Vec<_>>(),
            "order {order}"
        );
    }
}

#[test]
fn linear_predictors_of_many_orders_restore_the_signal() {
    let x = wave(300, 16, 3);
    for (order, precision, shift) in [
        (1, 8, 4),
        (2, 12, 9),
        (8, 15, 12),
        (12, 14, 10),
        (32, 15, 14),
        (32, 5, 0),
    ] {
        let coefs: Vec<i64> = (0..order)
            .map(|j| {
                let max = (1i64 << (precision - 1)) - 1;
                ((j as i64 * 37 + 11) % (2 * max + 1)) - max
            })
            .collect();
        let f = mono_frame(&x, 16, |w| sub_lpc(w, &coefs, precision, shift, &x, 16));
        let out = decode(&stream(44100, 1, 16), &f).unwrap();
        assert_eq!(
            s16(&out),
            x.iter().map(|&v| v as i16).collect::<Vec<_>>(),
            "order {order}"
        );
    }
}

#[test]
fn an_order_equal_to_the_block_size_has_no_residual() {
    let x = wave(16, 16, 4);
    let coefs = [3i64, -1];
    let f = mono_frame(&x, 16, |w| sub_lpc(w, &coefs, 4, 1, &x, 16));
    assert_eq!(s16(&decode(&stream(8000, 1, 16), &f).unwrap()).len(), 16);
    let x = wave(4, 16, 4);
    let f = mono_frame(&x, 16, |w| sub_fixed(w, 4, &x, 16, 0));
    assert_eq!(s16(&decode(&stream(8000, 1, 16), &f).unwrap()).len(), 4);
}

#[test]
fn wasted_bits_are_shifted_back_in() {
    let x: Vec<i64> = wave(64, 16, 5).iter().map(|v| v & !0x1F).collect();
    let f = mono_frame(&x, 16, |w| sub_fixed(w, 2, &x, 16, 5));
    let out = decode(&stream(44100, 1, 16), &f).unwrap();
    assert_eq!(s16(&out), x.iter().map(|&v| v as i16).collect::<Vec<_>>());
    let f = mono_frame(&x, 16, |w| {
        sub_header(w, 1, 5);
        for &v in &x {
            w.write_signed(v >> 5, 11);
        }
    });
    assert_eq!(
        s16(&decode(&stream(44100, 1, 16), &f).unwrap()),
        x.iter().map(|&v| v as i16).collect::<Vec<_>>()
    );
    // A constant that is all wasted bits.
    let f = mono_frame(&[0; 20], 16, |w| {
        sub_header(w, 0, 3);
        w.write_signed(-5, 13);
    });
    assert_eq!(
        s16(&decode(&stream(44100, 1, 16), &f).unwrap()),
        vec![-40i16; 20]
    );
}

// ---- residual coding ----

fn residual_frame(
    five_bit: bool,
    order: u32,
    parts: &[Part],
    res: &[i64],
    block: usize,
    pred: usize,
) -> Vec<u8> {
    let warm = vec![0i64; pred];
    mono_frame(&vec![0; block], 16, |w| {
        sub_header(w, 8 + pred as u64, 0);
        for &v in &warm {
            w.write_signed(v, 16);
        }
        write_residual(w, five_bit, order, block, pred, res, parts);
    })
}

/// With zero warm-up and an order-0 predictor the residuals are the samples.
fn zero_order(five_bit: bool, order: u32, parts: &[Part], res: &[i64], block: usize) -> Vec<i16> {
    let f = residual_frame(five_bit, order, parts, res, block, 0);
    s16(&decode(&stream(44100, 1, 16), &f).unwrap())
}

#[test]
fn rice_coded_residuals_with_both_parameter_widths_and_partition_orders() {
    let res = wave(256, 12, 6);
    let want: Vec<i16> = res.iter().map(|&v| v as i16).collect();
    for five in [false, true] {
        for order in 0..=4u32 {
            for k in [0u32, 1, 5, 11] {
                let parts = vec![Part::Rice(k); 1 << order];
                assert_eq!(
                    zero_order(five, order, &parts, &res, 256),
                    want,
                    "five={five} order={order} k={k}"
                );
            }
        }
    }
    // The 5-bit width reaches parameters above 14.
    let big: Vec<i64> = (0..64).map(|i| (i - 32) * 500).collect();
    let parts = vec![Part::Rice(20)];
    let f = residual_frame(true, 0, &parts, &big, 64, 0);
    let out = s16(&decode(&stream(44100, 1, 16), &f).unwrap());
    assert_eq!(out, big.iter().map(|&v| v as i16).collect::<Vec<_>>());
}

#[test]
fn the_first_partition_gives_up_the_predictor_order_samples() {
    let x = wave(64, 16, 7);
    let order = 2usize;
    let res: Vec<i64> = (order..64)
        .map(|i| x[i] - fixed_prediction(order, &x[..i]))
        .collect();
    // Partition order 2: four partitions of 16, the first holding 14 residuals.
    let f = mono_frame(&x, 16, |w| {
        sub_header(w, 8 + order as u64, 0);
        for &v in &x[..order] {
            w.write_signed(v, 16);
        }
        write_residual(w, false, 2, 64, order, &res, &[Part::Rice(9); 4]);
    });
    assert_eq!(
        s16(&decode(&stream(44100, 1, 16), &f).unwrap()),
        x.iter().map(|&v| v as i16).collect::<Vec<_>>()
    );
}

#[test]
fn escaped_partitions_hold_raw_residuals_and_zero_bits_mean_zeros() {
    let mut res = vec![0i64; 8];
    res.extend((0..8).map(|i| i - 4));
    res.extend((0..8).map(|i| (i - 4) * 1000));
    res.extend(wave(8, 8, 1));
    let want: Vec<i16> = res.iter().map(|&v| v as i16).collect();
    let parts = [
        Part::Escape(0),
        Part::Escape(3),
        Part::Escape(14),
        Part::Rice(4),
    ];
    assert_eq!(zero_order(false, 2, &parts, &res, 32), want);
    assert_eq!(zero_order(true, 2, &parts, &res, 32), want);
}

#[test]
fn partition_order_15_works_with_one_sample_per_partition() {
    let block = 1 << 15;
    let res: Vec<i64> = (0..block).map(|i| (i % 5) as i64 - 2).collect();
    let parts = vec![Part::Rice(1); block];
    let f = mono_frame(&vec![0; block], 16, |w| {
        sub_header(w, 8, 0);
        write_residual(w, false, 15, block, 0, &res, &parts);
    });
    let out = s16(&decode(&stream(44100, 1, 16), &f).unwrap());
    assert_eq!(out, res.iter().map(|&v| v as i16).collect::<Vec<_>>());
}

// ---- stereo ----

fn stereo_frame(
    assign: ChannelAssignment,
    bps: u8,
    a: &[i64],
    b: &[i64],
    bps_a: u32,
    bps_b: u32,
) -> Vec<u8> {
    frame_around(
        a.len() as u32,
        assign,
        Some(bps),
        None,
        body(|w| {
            sub_verbatim(w, a, bps_a);
            sub_verbatim(w, b, bps_b);
        }),
    )
}

#[test]
fn the_three_stereo_modes_restore_left_and_right() {
    let l = wave(40, 16, 8);
    let r = wave(40, 16, 9);
    let side: Vec<i64> = l.iter().zip(&r).map(|(l, r)| l - r).collect();
    let mid: Vec<i64> = l.iter().zip(&r).map(|(l, r)| (l + r) >> 1).collect();
    let want: Vec<i16> = l
        .iter()
        .zip(&r)
        .flat_map(|(&l, &r)| [l as i16, r as i16])
        .collect();
    let info = stream(44100, 2, 16);
    let cases = [
        (ChannelAssignment::Independent(2), &l, &r, 16, 16),
        (ChannelAssignment::LeftSide, &l, &side, 16, 17),
        (ChannelAssignment::SideRight, &side, &r, 17, 16),
        (ChannelAssignment::MidSide, &mid, &side, 16, 17),
    ];
    for (assign, a, b, ba, bb) in cases {
        let f = stereo_frame(assign, 16, a, b, ba, bb);
        assert_eq!(s16(&decode(&info, &f).unwrap()), want, "{assign:?}");
    }
}

#[test]
fn the_side_channel_of_a_32_bit_stream_needs_33_bits() {
    let l = [
        i64::from(i32::MAX),
        i64::from(i32::MIN),
        5,
        -5,
        i64::from(i32::MAX),
    ];
    let r = [
        i64::from(i32::MIN),
        i64::from(i32::MAX),
        -5,
        5,
        i64::from(i32::MAX),
    ];
    let side: Vec<i64> = l.iter().zip(&r).map(|(l, r)| l - r).collect();
    let mid: Vec<i64> = l.iter().zip(&r).map(|(l, r)| (l + r) >> 1).collect();
    assert!(side.iter().any(|&s| s > i64::from(i32::MAX)));
    let want: Vec<i32> = l
        .iter()
        .zip(&r)
        .flat_map(|(&l, &r)| [l as i32, r as i32])
        .collect();
    let info = stream(48000, 2, 32);
    for (assign, a, b, ba, bb) in [
        (
            ChannelAssignment::LeftSide,
            l.to_vec(),
            side.clone(),
            32,
            33,
        ),
        (
            ChannelAssignment::SideRight,
            side.clone(),
            r.to_vec(),
            33,
            32,
        ),
        (ChannelAssignment::MidSide, mid, side, 32, 33),
    ] {
        let f = stereo_frame(assign, 32, &a, &b, ba, bb);
        assert_eq!(s32(&decode(&info, &f).unwrap()), want, "{assign:?}");
    }
}

#[test]
fn up_to_eight_independent_channels_interleave() {
    for n in [1u8, 3, 6, 8] {
        let chans: Vec<Vec<i64>> = (0..n).map(|c| wave(20, 16, i64::from(c) + 20)).collect();
        let f = frame_around(
            20,
            ChannelAssignment::Independent(n),
            Some(16),
            None,
            body(|w| chans.iter().for_each(|c| sub_verbatim(w, c, 16))),
        );
        let out = decode(&stream(44100, n, 16), &f).unwrap();
        let want: Vec<i16> = (0..20)
            .flat_map(|i| chans.iter().map(move |c| c[i] as i16))
            .collect::<Vec<_>>();
        assert_eq!(s16(&out), want, "{n} channels");
        assert_eq!(out.layout.channels(), u16::from(n));
    }
}

// ---- output formats ----

#[test]
fn samples_are_left_justified_in_the_container() {
    for bits in [4u8, 8, 9, 12, 16, 17, 20, 24, 25, 32] {
        let max = (1i64 << (bits - 1)) - 1;
        let x = [0, 1, -1, max, -max - 1, max / 2];
        // Depths a frame header cannot name are inherited from STREAMINFO.
        let code = matches!(bits, 8 | 12 | 16 | 20 | 24 | 32).then_some(bits);
        let f = frame_around(
            6,
            ChannelAssignment::Independent(1),
            code,
            None,
            body(|w| sub_verbatim(w, &x, u32::from(bits))),
        );
        let out = decode(&stream(48000, 1, bits), &f).unwrap();
        let shift = |container: u32| u32::from(bits).abs_diff(container);
        match (&out.samples, bits) {
            (Samples::U8(v), 4..=8) => {
                let want: Vec<u8> = x
                    .iter()
                    .map(|&s| ((s << (8 - u32::from(bits))) + 128) as u8)
                    .collect();
                assert_eq!(*v, want, "{bits}");
            }
            (Samples::S16(v), 9..=16) => {
                let want: Vec<i16> = x.iter().map(|&s| (s << shift(16)) as i16).collect();
                assert_eq!(*v, want, "{bits}");
            }
            (Samples::S24(v), 17..=24) => {
                let want: Vec<i32> = x.iter().map(|&s| (s << shift(24)) as i32).collect();
                assert_eq!(*v, want, "{bits}");
            }
            (Samples::S32(v), 25..=32) => {
                let want: Vec<i32> = x.iter().map(|&s| (s << shift(32)) as i32).collect();
                assert_eq!(*v, want, "{bits}");
            }
            other => panic!("{bits} bits gave {other:?}"),
        }
    }
}

// ---- rejections ----

#[test]
fn a_frame_that_changes_the_stream_parameters_is_unsupported() {
    let x = wave(16, 16, 1);
    let mono = mono_frame(&x, 16, |w| sub_verbatim(w, &x, 16));
    let info = stream(44100, 2, 16);
    let m = unsupported(decode(&info, &mono));
    assert!(
        m.contains("channel") && m.contains('1') && m.contains('2'),
        "{m}"
    );

    let m = unsupported(decode(&stream(44100, 1, 24), &mono));
    assert!(
        m.contains("bit depth") && m.contains("16") && m.contains("24"),
        "{m}"
    );

    let f = frame_around(
        16,
        ChannelAssignment::Independent(1),
        Some(16),
        Some(48000),
        body(|w| sub_verbatim(w, &x, 16)),
    );
    let m = unsupported(decode(&stream(44100, 1, 16), &f));
    assert!(
        m.contains("sample rate") && m.contains("48000") && m.contains("44100"),
        "{m}"
    );
}

#[test]
fn a_bad_crc_or_header_is_malformed() {
    let x = wave(16, 16, 1);
    let good = mono_frame(&x, 16, |w| sub_verbatim(w, &x, 16));
    let info = stream(44100, 1, 16);
    for at in [good.len() - 1, good.len() - 2, good.len() / 2] {
        let mut bad = good.clone();
        bad[at] ^= 0x10;
        assert!(malformed(decode(&info, &bad)).contains("CRC"), "byte {at}");
    }
    let mut bad = good.clone();
    bad[0] = 0;
    assert!(matches!(decode(&info, &bad), Err(Error::Malformed { .. })));
    assert!(matches!(
        decode(&info, &good[..4]),
        Err(Error::Malformed { .. } | Error::UnexpectedEof { .. })
    ));
    assert!(matches!(
        decode(&info, &[]),
        Err(Error::Malformed { .. } | Error::UnexpectedEof { .. })
    ));
}

#[test]
fn structural_violations_are_malformed() {
    let info = stream(44100, 1, 16);
    let x = vec![0i64; 32];
    let bad = |f: Vec<u8>| malformed(decode(&info, &f));

    // Reserved subframe types, a set pad bit, a reserved coding method.
    for kind in [13u64, 15, 16, 31] {
        let f = mono_frame(&x, 16, |w| sub_header(w, kind, 0));
        bad(f);
    }
    bad(mono_frame(&x, 16, |w| {
        w.write_bits(1, 1);
        w.write_bits(0, 7);
        w.write_bits(0, 16);
    }));
    bad(mono_frame(&x, 16, |w| {
        sub_header(w, 8, 0);
        w.write_bits(2, 2);
        w.write_bits(0, 4);
    }));
    // Predictor order above the block size.
    bad(frame_around(
        2,
        ChannelAssignment::Independent(1),
        Some(16),
        None,
        body(|w| {
            sub_header(w, 8 + 4, 0);
            for _ in 0..4 {
                w.write_signed(0, 16);
            }
        }),
    ));
    // Partition order that does not divide the block size, and one that leaves a partition
    // smaller than the predictor order.
    bad(frame_around(
        30,
        ChannelAssignment::Independent(1),
        Some(16),
        None,
        body(|w| {
            sub_header(w, 8, 0);
            w.write_bits(0, 2);
            w.write_bits(2, 4);
            w.write_bits(0, 64);
        }),
    ));
    bad(mono_frame(&x, 16, |w| {
        sub_header(w, 8 + 4, 0);
        for _ in 0..4 {
            w.write_signed(0, 16);
        }
        w.write_bits(0, 2);
        w.write_bits(4, 4); // 32 >> 4 = 2 samples per partition, fewer than 4 warm-up samples
        w.write_bits(0, 64);
    }));
    // LPC: precision code 15 and a negative shift.
    bad(mono_frame(&x, 16, |w| {
        sub_header(w, 31 + 1, 0);
        w.write_signed(0, 16);
        w.write_bits(15, 4);
        w.write_bits(0, 40);
    }));
    let m = bad(mono_frame(&x, 16, |w| {
        sub_header(w, 31 + 1, 0);
        w.write_signed(0, 16);
        w.write_bits(7, 4);
        w.write_signed(-1, 5);
        w.write_signed(1, 8);
        write_residual(w, false, 0, 32, 1, &[0; 31], &rice_parts(0));
    }));
    assert!(m.contains("shift"), "{m}");
    // Wasted bits that swallow the whole sample.
    bad(mono_frame(&x, 16, |w| {
        sub_header(w, 0, 16);
        w.write_bits(0, 16);
    }));
    // Bytes left over before the CRC.
    bad(mono_frame(&x, 16, |w| {
        sub_constant(w, 3, 16);
        w.write_bits(0xAB, 8);
    }));
    // The subframe data runs out.
    bad(mono_frame(&x, 16, |w| sub_verbatim(w, &x[..10], 16)));
}

#[test]
fn a_bad_stream_description_is_rejected() {
    let mut info = stream(44100, 2, 16);
    info.extradata.pop();
    assert!(matches!(decoder(&info), Err(Error::Malformed { .. })));
    let mut info = stream(44100, 2, 16);
    info.extradata.clear();
    assert!(matches!(decoder(&info), Err(Error::Malformed { .. })));
    // The description and STREAMINFO disagree.
    let mut info = stream(44100, 2, 16);
    info.extradata = block_info(44100, 1, 16, 0, [0; 16]).write().to_vec();
    assert!(matches!(decoder(&info), Err(Error::Malformed { .. })));
}

// ---- MD5 and totals ----

fn md5_of(samples: &[i64], bytes: usize) -> [u8; 16] {
    let mut m = Md5::new();
    for s in samples {
        m.update(&s.to_le_bytes()[..bytes]);
    }
    m.finalize()
}

fn run(info: &StreamInfo, frames: &[Vec<u8>]) -> (Vec<Result<bool, Error>>, usize) {
    let mut d = decoder(info).unwrap();
    let mut out = AudioFrame::new(&info.audio);
    let mut got = 0;
    for f in frames {
        d.send_packet(&Packet {
            data: f.clone(),
            ..Packet::default()
        })
        .unwrap();
        while d.receive_frame(&mut out).unwrap() {
            got += 1;
        }
    }
    d.drain();
    let mut tail = Vec::new();
    for _ in 0..3 {
        tail.push(d.receive_frame(&mut out));
    }
    (tail, got)
}

fn two_frames() -> (Vec<Vec<u8>>, Vec<i64>) {
    let l = wave(48, 12, 30);
    let r = wave(48, 12, 31);
    let mut frames = Vec::new();
    let mut inter = Vec::new();
    for range in [0..30, 30..48] {
        let (a, b) = (&l[range.clone()], &r[range]);
        frames.push(stereo_frame(
            ChannelAssignment::Independent(2),
            12,
            a,
            b,
            12,
            12,
        ));
        for (x, y) in a.iter().zip(b) {
            inter.extend([*x, *y]);
        }
    }
    (frames, inter)
}

#[test]
fn a_matching_md5_and_total_end_cleanly() {
    let (frames, inter) = two_frames();
    // The hash is over the raw 12-bit values in two bytes each, not the shifted output.
    let info = stream_for(&block_info(44100, 2, 12, 48, md5_of(&inter, 2)));
    let (tail, got) = run(&info, &frames);
    assert_eq!(got, 2);
    assert_eq!(tail, vec![Ok(false), Ok(false), Ok(false)]);
}

#[test]
fn a_wrong_md5_is_reported_once() {
    let (frames, inter) = two_frames();
    let mut md5 = md5_of(&inter, 2);
    md5[3] ^= 1;
    let info = stream_for(&block_info(44100, 2, 12, 48, md5));
    let (tail, _) = run(&info, &frames);
    assert_eq!(
        tail,
        vec![
            Err(Error::Malformed {
                what: "flac: decoded audio does not match the MD5 in the stream header".into()
            }),
            Ok(false),
            Ok(false)
        ]
    );
}

#[test]
fn a_zero_md5_and_a_zero_total_mean_no_check() {
    let (frames, _) = two_frames();
    let info = stream_for(&block_info(44100, 2, 12, 0, [0; 16]));
    let (tail, got) = run(&info, &frames);
    assert_eq!((got, tail), (2, vec![Ok(false); 3]));
}

#[test]
fn a_total_that_does_not_match_is_reported() {
    let (frames, _) = two_frames();
    let info = stream_for(&block_info(44100, 2, 12, 100, [0; 16]));
    let (tail, _) = run(&info, &frames);
    assert_eq!(
        tail[0],
        Err(Error::Malformed {
            what: "flac: stream ended after 48 of 100 samples".into()
        })
    );
    assert_eq!(tail[1], Ok(false));
    let info = stream_for(&block_info(44100, 2, 12, 10, [0; 16]));
    let (tail, _) = run(&info, &frames);
    assert_eq!(
        tail[0],
        Err(Error::Malformed {
            what: "flac: stream ended after 48 of 10 samples".into()
        })
    );
}

#[test]
fn md5_of_wide_and_narrow_samples_uses_the_raw_width() {
    for bits in [8u8, 20, 32] {
        let x = wave(32, u32::from(bits), 40);
        let f = frame_around(
            32,
            ChannelAssignment::Independent(1),
            Some(bits),
            None,
            body(|w| sub_verbatim(w, &x, u32::from(bits))),
        );
        let bytes = usize::from(bits).div_ceil(8);
        let info = stream_for(&block_info(8000, 1, bits, 32, md5_of(&x, bytes)));
        let (tail, _) = run(&info, std::slice::from_ref(&f));
        assert_eq!(tail[0], Ok(false), "{bits} bits");
    }
}

#[test]
fn a_failed_frame_suppresses_the_end_of_stream_checks() {
    let (frames, _) = two_frames();
    let info = stream_for(&block_info(44100, 2, 12, 48, [9; 16]));
    let mut d = decoder(&info).unwrap();
    let mut bad = frames[0].clone();
    *bad.last_mut().unwrap() ^= 1;
    let p = |data: &[u8]| Packet {
        data: data.to_vec(),
        ..Packet::default()
    };
    assert!(d.send_packet(&p(&bad)).is_err());
    d.drain();
    let mut out = AudioFrame::new(&info.audio);
    assert_eq!(d.receive_frame(&mut out), Ok(false));
}

#[test]
fn frames_are_handed_over_without_reallocating() {
    let x = wave(256, 16, 50);
    let f = mono_frame(&x, 16, |w| sub_verbatim(w, &x, 16));
    let info = stream(44100, 1, 16);
    let mut d = decoder(&info).unwrap();
    let mut out = AudioFrame::new(&info.audio);
    let mut seen = std::collections::HashSet::new();
    for i in 0..10 {
        d.send_packet(&Packet {
            data: f.clone(),
            pts: i,
            ..Packet::default()
        })
        .unwrap();
        assert!(d.receive_frame(&mut out).unwrap());
        assert_eq!(out.pts, i);
        let Samples::S16(v) = &out.samples else {
            panic!()
        };
        seen.insert(v.as_ptr() as usize);
        assert_eq!(v.len(), 256);
    }
    // Two buffers trade places; nothing new appears after the start.
    assert!(seen.len() <= 2, "{} distinct buffers", seen.len());
    assert_eq!(d.receive_frame(&mut out), Ok(false));
}

// ---- hostile input ----

proptest! {
    #[test]
    fn a_valid_envelope_around_random_bytes_never_panics(
        data in proptest::collection::vec(any::<u8>(), 0..400),
        block in 1u32..300,
        assign in 0u8..11,
        bits in prop::sample::select(vec![8u8, 16, 24, 32]),
    ) {
        let channels = match assign {
            0..=7 => ChannelAssignment::Independent(1 + assign.min(1)),
            8 => ChannelAssignment::LeftSide,
            9 => ChannelAssignment::SideRight,
            _ => ChannelAssignment::MidSide,
        };
        let n = channels.channels();
        let f = frame_around(block, channels, Some(bits), None, data);
        let _ = decode(&stream(44100, n, bits), &f);
    }

    #[test]
    fn flipping_bits_of_a_good_frame_never_panics(flips in proptest::collection::vec((0usize..400, 0u8..8), 1..6)) {
        let l = wave(64, 16, 60);
        let r = wave(64, 16, 61);
        let mut f = stereo_frame(ChannelAssignment::Independent(2), 16, &l, &r, 16, 16);
        for (at, bit) in flips {
            let i = at % f.len();
            f[i] ^= 1 << bit;
        }
        // Re-seal the trailer so the corruption reaches the subframe parser.
        let n = f.len();
        let crc = crc16(&f[..n - 2]);
        f[n - 2..].copy_from_slice(&crc.to_be_bytes());
        let _ = decode(&stream(44100, 2, 16), &f);
    }
}
