//! Whether a source layout can be written as the one layout a codec stores for its channel
//! count: the same, the same speakers under the other surround name, no labels at all, or not.

use crossfade_core::{ChannelLayout, Fit};

fn named(name: &str) -> ChannelLayout {
    ChannelLayout::from_name(name).unwrap_or_else(|| panic!("{name}"))
}

fn mask(mask: u32) -> ChannelLayout {
    ChannelLayout::from_mask(mask, mask.count_ones() as u16).unwrap()
}

#[test]
fn the_same_layout_fits_itself() {
    for name in ChannelLayout::NAMES {
        assert_eq!(named(name).fit_to(&named(name)), Fit::Same, "{name}");
    }
}

#[test]
fn one_surround_pair_under_the_other_name_is_the_same_speakers() {
    for (side, back) in [("quad-side", "quad"), ("5.0", "5.0-back"), ("5.1", "5.1-back")] {
        assert_eq!(named(side).fit_to(&named(back)), Fit::Renamed, "{side}");
        assert_eq!(named(back).fit_to(&named(side)), Fit::Renamed, "{back}");
    }
}

#[test]
fn only_the_surround_name_may_differ() {
    // Both pairs present: 7.1 has nothing to be renamed to.
    assert_eq!(named("7.1").fit_to(&named("5.1")), Fit::Different);
    // ALAC's 8 channels (front-of-centre pair, side pair) are not standard 7.1.
    let alac8 = mask(0x6CF);
    assert_eq!(alac8.fit_to(&named("7.1")), Fit::Different);
    assert_eq!(named("7.1").fit_to(&alac8), Fit::Different);
    // A different centre or LFE set.
    assert_eq!(named("5.1").fit_to(&named("5.0-back")), Fit::Different);
    assert_eq!(mask(0x60B).fit_to(&named("5.0-back")), Fit::Different);
    assert_eq!(named("4.0").fit_to(&named("quad")), Fit::Different);
    assert_eq!(named("quad").fit_to(&named("4.0")), Fit::Different);
    // The same speakers in another order are not a rename.
    assert_eq!(mask(0x13F).fit_to(&named("6.1")), Fit::Different);
    // Half a pair, or a different count.
    assert_eq!(mask(0x20F).fit_to(&named("5.1-back")), Fit::Different);
    assert_eq!(named("5.1").fit_to(&named("5.0")), Fit::Different);
}

#[test]
fn channels_with_no_labels_take_the_layout_asked_for() {
    for n in [3u16, 4, 5, 6, 7, 8] {
        let source = ChannelLayout::unpositioned(n).unwrap();
        let wanted = ChannelLayout::default_for(n).unwrap();
        assert_eq!(source.fit_to(&wanted), Fit::Unlabelled, "{n}");
    }
    let six = ChannelLayout::unpositioned(6).unwrap();
    assert_eq!(six.fit_to(&named("5.0")), Fit::Different);
    assert_eq!(six.fit_to(&six), Fit::Same);
    assert_eq!(named("5.1").fit_to(&six), Fit::Different);
}
