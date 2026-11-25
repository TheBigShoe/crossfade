use super::*;
use crate::range::RangeDecoder;
use crate::silk::decode_indices::decode_indices;
use crate::silk::state::ChannelState;
use crate::silk::tables::{
    PITCH_CONTOUR_10_MS_ICDF, PITCH_CONTOUR_10_MS_NB_ICDF, PITCH_CONTOUR_ICDF,
    PITCH_CONTOUR_NB_ICDF, UNIFORM6_ICDF,
};
use crate::silk::tables_nlsf::{NLSF_CB_NB_MB, NLSF_CB_WB};

fn rng(seed: u32) -> impl FnMut(u32) -> u32 {
    let mut s = seed.wrapping_mul(2_654_435_761) | 1;
    move |n| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s % n
    }
}

#[test]
fn indices_decode_back_with_the_same_final_range() {
    let mut r = rng(3);
    for case in 0..600 {
        let fs_khz = [8, 12, 16][case % 3];
        let nb_subfr = if (case / 3) % 2 == 0 { 4 } else { 2 };
        let cb = if fs_khz == 16 {
            &NLSF_CB_WB
        } else {
            &NLSF_CB_NB_MB
        };
        let low_bits: &'static [u8] = match fs_khz {
            8 => &UNIFORM4_ICDF,
            12 => &UNIFORM6_ICDF,
            _ => &UNIFORM8_ICDF,
        };
        let contour: &'static [u8] = match (fs_khz, nb_subfr) {
            (8, 4) => &PITCH_CONTOUR_NB_ICDF,
            (8, _) => &PITCH_CONTOUR_10_MS_NB_ICDF,
            (_, 4) => &PITCH_CONTOUR_ICDF,
            _ => &PITCH_CONTOUR_10_MS_ICDF,
        };
        let cond = [
            CondCoding::Independently,
            CondCoding::IndependentlyNoLtpScaling,
            CondCoding::Conditionally,
        ][(case / 6) % 3];
        let vad = case % 5 != 0;
        let mut ix = SideInfoIndices {
            signal_type: if vad { 1 + r(2) as i8 } else { 0 },
            quant_offset_type: r(2) as i8,
            ..SideInfoIndices::default()
        };
        ix.gains_indices[0] = if cond == CondCoding::Conditionally {
            r(41) as i8
        } else {
            r(64) as i8
        };
        for g in ix.gains_indices[1..].iter_mut() {
            *g = r(41) as i8;
        }
        ix.nlsf_indices[0] = r(32) as i8;
        for v in ix.nlsf_indices[1..=cb.order].iter_mut() {
            *v = r(21) as i8 - 10;
        }
        ix.nlsf_interp_coef_q2 = if nb_subfr == 4 { r(5) as i8 } else { 4 };
        ix.lag_index = r(((18 - 2) * fs_khz) as u32) as i16;
        ix.contour_index = r((contour.len()) as u32) as i8;
        ix.per_index = r(3) as i8;
        for l in ix.ltp_index.iter_mut() {
            *l = r(8 << ix.per_index) as i8;
        }
        ix.ltp_scale_index = if cond == CondCoding::Independently {
            r(3) as i8
        } else {
            0
        };
        ix.seed = r(4) as i8;
        let prev_voiced = case % 7 < 4;
        let prev_lag = (i32::from(ix.lag_index) + r(30) as i32 - 15).max(0) as i16;

        let mut prev_type = if prev_voiced { TYPE_VOICED } else { 1 };
        let mut prev_lag_enc = prev_lag;
        let mut c = IndexCoder {
            nb_subfr,
            fs_khz,
            nlsf_cb: cb,
            pitch_lag_low_bits_icdf: low_bits,
            pitch_contour_icdf: contour,
            ec_prev_signal_type: &mut prev_type,
            ec_prev_lag_index: &mut prev_lag_enc,
        };
        let mut enc = RangeEncoder::new(1275);
        encode_indices(&mut c, &ix, &mut enc, false, cond);
        let rng_enc = enc.rng();
        let bytes = enc.done().expect("fits");

        let mut st = ChannelState::new();
        st.fs_khz = fs_khz;
        st.nb_subfr = nb_subfr;
        st.nlsf_cb = cb;
        st.pitch_lag_low_bits_icdf = low_bits;
        st.pitch_contour_icdf = contour;
        st.vad_flags[0] = vad;
        st.ec_prev_signal_type = if prev_voiced { TYPE_VOICED } else { 1 };
        st.ec_prev_lag_index = prev_lag;
        let mut dec = RangeDecoder::new(&bytes);
        decode_indices(&mut st, &mut dec, 0, false, cond);
        let d = &st.indices;
        let n = nb_subfr;
        assert_eq!(d.signal_type, ix.signal_type, "case {case}");
        assert_eq!(d.quant_offset_type, ix.quant_offset_type);
        assert_eq!(d.gains_indices[..n], ix.gains_indices[..n], "case {case}");
        assert_eq!(d.nlsf_indices[..=cb.order], ix.nlsf_indices[..=cb.order]);
        assert_eq!(d.nlsf_interp_coef_q2, ix.nlsf_interp_coef_q2);
        if ix.signal_type == 2 {
            assert_eq!(d.lag_index, ix.lag_index, "case {case}");
            assert_eq!(d.contour_index, ix.contour_index);
            assert_eq!(d.per_index, ix.per_index);
            assert_eq!(d.ltp_index[..n], ix.ltp_index[..n]);
            assert_eq!(d.ltp_scale_index, ix.ltp_scale_index);
        }
        assert_eq!(d.seed, ix.seed);
        assert_eq!(dec.rng(), rng_enc, "case {case}: final range");
        assert_eq!(st.ec_prev_lag_index, prev_lag_enc);
    }
}
