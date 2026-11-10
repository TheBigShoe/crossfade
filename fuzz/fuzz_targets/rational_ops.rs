#![no_main]

use arbitrary::Arbitrary;
use crossfade_core::Rational;
use libfuzzer_sys::fuzz_target;

#[derive(Debug, Arbitrary)]
struct Input {
    a: (i64, i64),
    b: (i64, i64),
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn assert_normalised(r: Rational) {
    assert!(r.den() > 0, "non-positive denominator in {r:?}");
    assert_eq!(
        gcd(
            u128::from(r.num().unsigned_abs()),
            u128::from(r.den().unsigned_abs())
        ),
        1,
        "not reduced: {r:?}"
    );
}

fuzz_target!(|input: Input| {
    let (Ok(a), Ok(b)) = (
        Rational::new(input.a.0, input.a.1),
        Rational::new(input.b.0, input.b.1),
    ) else {
        return;
    };
    assert_normalised(a);
    assert_normalised(b);

    let add = a.checked_add(b);
    let mul = a.checked_mul(b);
    for r in [add.clone(), a.checked_sub(b), mul.clone(), a.checked_div(b)]
        .into_iter()
        .flatten()
    {
        assert_normalised(r);
    }

    if let (Ok(x), Ok(y)) = (add, b.checked_add(a)) {
        assert_eq!(x, y, "addition not commutative");
    }
    if let (Ok(x), Ok(y)) = (mul, b.checked_mul(a)) {
        assert_eq!(x, y, "multiplication not commutative");
    }

    assert_eq!(a.cmp(&b), b.cmp(&a).reverse());
});
