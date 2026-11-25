use super::encode_pulses;
use crate::range::RangeDecoder;
use crate::range_enc::RangeEncoder;
use crate::silk::decode_pulses::decode_pulses;
use crate::silk::state::MAX_FRAME_LENGTH;

/// xorshift32: the same pseudo-random numbers on every platform.
fn rng(seed: u32) -> impl FnMut() -> u32 {
    let mut s = seed.wrapping_mul(2_654_435_761) | 1;
    move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    }
}

#[test]
fn pulses_decode_back_with_the_same_final_range() {
    let mut next = rng(7);
    // Every frame length (10 and 20 ms at 8, 12, 16 kHz), signal type and offset; sparse,
    // dense and huge pulses (the huge ones need the LSB escape).
    for (case, frame_length) in [80usize, 120, 160, 240, 320]
        .into_iter()
        .cycle()
        .take(60)
        .enumerate()
    {
        let signal_type = (case % 3) as i32;
        let quant_offset_type = ((case / 3) % 2) as i32;
        let amp = [1u32, 3, 8, 30, 127][case % 5];
        let mut pulses = [0i8; MAX_FRAME_LENGTH];
        for p in pulses.iter_mut().take(frame_length) {
            let r = next();
            if !r.is_multiple_of(4) {
                let mag = (r >> 8) % (amp + 1);
                *p = if r & 0x80 != 0 {
                    -(mag as i8)
                } else {
                    mag as i8
                };
            }
        }
        let want = pulses;
        let mut enc = RangeEncoder::new(1275);
        encode_pulses(
            &mut enc,
            signal_type,
            quant_offset_type,
            &mut pulses,
            frame_length,
        );
        let rng_enc = enc.rng();
        let bytes = enc.done().expect("fits");
        let mut dec = RangeDecoder::new(&bytes);
        let mut got = [0i16; MAX_FRAME_LENGTH];
        decode_pulses(
            &mut dec,
            &mut got,
            signal_type,
            quant_offset_type,
            frame_length,
        );
        for i in 0..frame_length {
            assert_eq!(i16::from(want[i]), got[i], "case {case} sample {i}");
        }
        assert_eq!(dec.rng(), rng_enc, "case {case}: final range");
    }
}
