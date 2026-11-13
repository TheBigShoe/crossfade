use super::*;

/// The C definitions, transcribed literally with wrapping arithmetic.
fn c_smulwb(a: i32, b: i32) -> i32 {
    let b = i32::from(b as i16);
    (a >> 16)
        .wrapping_mul(b)
        .wrapping_add(((a & 0xFFFF).wrapping_mul(b)) >> 16)
}

fn c_smulww(a: i32, b: i32) -> i32 {
    c_smulwb(a, b).wrapping_add(a.wrapping_mul(rshift_round(b, 16)))
}

const SAMPLES: [i32; 14] = [
    0,
    1,
    -1,
    2,
    32767,
    -32768,
    65535,
    65536,
    -65537,
    123_456_789,
    -987_654_321,
    i32::MAX,
    i32::MIN,
    0x7fff_8000,
];

#[test]
fn multiply_and_shift_equal_the_c_macros() {
    for &a in &SAMPLES {
        for &b in &SAMPLES {
            assert_eq!(smulwb(a, b), c_smulwb(a, b), "smulwb {a} {b}");
            assert_eq!(smulww(a, b), c_smulww(a, b), "smulww {a} {b}");
            assert_eq!(smlawb(7, a, b), 7i32.wrapping_add(c_smulwb(a, b)));
            assert_eq!(smulbb(a, b), i32::from(a as i16) * i32::from(b as i16));
        }
    }
}

#[test]
fn rounding_shifts_round_half_up() {
    assert_eq!(rshift_round(3, 1), 2);
    assert_eq!(rshift_round(-3, 1), -1);
    assert_eq!(rshift_round(0x18000, 16), 2);
    assert_eq!(rshift_round(-0x18000, 16), -1);
    assert_eq!(rshift_round(i32::MAX, 8), 1 << 23);
    assert_eq!(rshift_round64(-5, 1), -2);
    assert_eq!(rshift_round64(i64::MAX, 32), 1 << 31);
}

#[test]
fn saturation_and_limits() {
    assert_eq!(sat16(40000), 32767);
    assert_eq!(sat16(-40000), -32768);
    assert_eq!(add_sat16(32000, 1000), 32767);
    assert_eq!(limit(5, 10, 0), 5);
    assert_eq!(limit(15, 10, 0), 10);
    assert_eq!(limit(-5, 0, 10), 0);
    // Limited before the shift, as the C does: not quite i32::MAX.
    assert_eq!(lshift_sat32(1 << 20, 12), 0x7FFF_F000);
    assert_eq!(lshift_sat32(-(1 << 20), 12), i32::MIN);
    assert_eq!(lshift_sat32(3, 4), 48);
}

#[test]
fn clz_and_fraction() {
    assert_eq!(clz32(0), 32);
    assert_eq!(clz32(1), 31);
    assert_eq!(clz32(-1), 0);
    // 0b1011_0110_1 << 20: the seven bits after the leading one.
    assert_eq!(clz_frac(0b1011_0110 << 20), (4, 0b011_0110));
    assert_eq!(clz_frac(5), (29, 0b010_0000));
}

#[test]
fn square_root_and_divisions_approximate() {
    for x in [16, 1000, 65536, 1 << 20, 123_456_789] {
        let want = f64::from(x).sqrt();
        let got = f64::from(sqrt_approx(x));
        assert!((got - want).abs() <= want * 0.1, "sqrt {x}: {got}");
    }
    assert_eq!(sqrt_approx(0), 0);
    assert_eq!(sqrt_approx(-4), 0);
    for (a, b, q) in [(1 << 16, 3 << 16, 16), (65536, 81920, 16), (-1000, 7, 10)] {
        let want = (f64::from(a) * f64::from(1 << q)) / f64::from(b);
        let got = f64::from(div32_varq(a, b, q));
        assert!(
            (got - want).abs() <= want.abs() * 1e-3 + 1.0,
            "{a}/{b}: {got}"
        );
    }
    for (b, q) in [(65536, 47), (81920, 47), (1 << 30, 46), (100_000, 31)] {
        let want = 2f64.powi(q) / f64::from(b);
        let got = f64::from(inverse32_varq(b, q));
        assert!((got - want).abs() <= want * 1e-3 + 1.0, "1/{b}: {got}");
    }
    assert_eq!(div32(5, 0), i32::MAX);
    assert_eq!(div32(i32::MIN, -1), i32::MIN);
}

#[test]
fn log2lin_and_rand() {
    assert_eq!(log2lin(-1), 0);
    assert_eq!(log2lin(0), 1);
    assert_eq!(log2lin(7 << 7), 128);
    assert_eq!(log2lin(3967), 2_130_706_432);
    assert_eq!(log2lin(i32::MAX), i32::MAX);
    assert_eq!(silk_rand(0), 907_633_515);
    assert_eq!(silk_rand(1), 907_633_515 + 196_314_165);
}

#[test]
fn energy_with_headroom() {
    assert_eq!(sum_sqr_shift(&[]), (0, 0));
    assert_eq!(sum_sqr_shift(&[3, 4]), (25, 0));
    assert_eq!(sum_sqr_shift(&[1, 2, 3]), (14, 0));
    let loud = [i16::MIN; 320];
    let (nrg, shift) = sum_sqr_shift(&loud);
    assert!(nrg > 0 && nrg < 1 << 29, "{nrg}");
    assert_eq!(i64::from(nrg) << shift, 320i64 << 30);
}
