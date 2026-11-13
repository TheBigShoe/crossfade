use super::*;

#[test]
fn isqrt32_is_the_floor_of_the_square_root() {
    for v in (0..100_000u32).chain([u32::MAX, u32::MAX - 1, 1 << 31, 0xFFFE_0001]) {
        let g = u64::from(isqrt32(v));
        assert!(
            g * g <= u64::from(v) && (g + 1) * (g + 1) > u64::from(v),
            "{v}"
        );
    }
}

#[test]
fn bitexact_cos_is_the_reference_curve() {
    // Endpoints and the midpoint, worked by hand from the reference's formula.
    // Angles are multiples of 16384 / qn, qn <= 256: never below 64.
    assert_eq!(bitexact_cos(64), 32767);
    assert_eq!(bitexact_cos(8192), 23171);
    // Monotone falling over the quarter turn (16384 itself is never asked: the callers
    // special-case it).
    assert!(bitexact_cos(16383) < 10);
    for x in 65..16384 {
        assert!(bitexact_cos(x) <= bitexact_cos(x - 1), "{x}");
    }
}

#[test]
fn bitexact_log2tan_is_antisymmetric_and_zero_at_45_degrees() {
    assert_eq!(bitexact_log2tan(23171, 23171), 0);
    for (s, c) in [(1000, 30000), (12345, 23456), (32767, 1)] {
        assert_eq!(bitexact_log2tan(s, c), -bitexact_log2tan(c, s));
    }
    // log2(tan) of a 2:1 ratio is about one octave: 2048 in Q11.
    assert!((bitexact_log2tan(20000, 10000) - 2048).abs() < 40);
}

#[test]
fn transcendentals_are_the_double_forms() {
    assert_eq!(exp2(0.0), 1.0);
    assert_eq!(exp2(10.0), 1024.0);
    assert_eq!(cos_norm(0.0), 1.0);
    assert!(cos_norm(1.0).abs() < 1e-7);
    assert_eq!(lcg_rand(0), 1_013_904_223);
}
