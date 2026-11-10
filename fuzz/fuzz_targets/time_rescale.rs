#![no_main]

use arbitrary::Arbitrary;
use crossfade_core::{Rounding, TimeBase, Timestamp};
use libfuzzer_sys::fuzz_target;
use std::cmp::Ordering;

#[derive(Debug, Arbitrary)]
struct Input {
    ticks: i64,
    other_ticks: i64,
    from: (i64, i64),
    to: (i64, i64),
}

const ALL: [Rounding; 4] = [
    Rounding::NearestAwayFromZero,
    Rounding::Floor,
    Rounding::Ceil,
    Rounding::TowardZero,
];

fuzz_target!(|input: Input| {
    let (Ok(from), Ok(to)) = (
        TimeBase::new(input.from.0, input.from.1),
        TimeBase::new(input.to.0, input.to.1),
    ) else {
        return;
    };
    let x = Timestamp::new(input.ticks, from);
    let y = Timestamp::new(input.other_ticks, to);

    for r in ALL {
        if let Ok(same) = x.rescale(from, r) {
            assert_eq!(
                same.ticks(),
                input.ticks,
                "identity rescale changed ticks ({r:?})"
            );
        } else {
            panic!("rescale to the same base failed ({r:?})");
        }
    }

    let floor = x.rescale(to, Rounding::Floor);
    let ceil = x.rescale(to, Rounding::Ceil);
    let zero = x.rescale(to, Rounding::TowardZero);
    let near = x.rescale(to, Rounding::NearestAwayFromZero);

    if let (Ok(f), Ok(c), Ok(z), Ok(n)) = (&floor, &ceil, &zero, &near) {
        let (f, c, z, n) = (f.ticks(), c.ticks(), z.ticks(), n.ticks());
        assert!(f <= z && z <= c, "TowardZero outside [Floor, Ceil]");
        assert!(f <= n && n <= c, "Nearest outside [Floor, Ceil]");
        assert!(i128::from(c) - i128::from(f) <= 1, "Ceil - Floor > 1");
    }

    if let Ok(f) = floor.as_ref() {
        assert!(*f <= x, "Floor rescale exceeded the original");
    }
    if let Ok(c) = ceil.as_ref() {
        assert!(*c >= x, "Ceil rescale fell below the original");
    }

    assert_eq!(x.cmp(&y), y.cmp(&x).reverse());
    assert_eq!(x.cmp(&x), Ordering::Equal);
});
