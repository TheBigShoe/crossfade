//! Speaker layouts by name: what `--channels` and `mix` accept and what `probe` can print.
//! The names follow FFmpeg's (with `-back`/`-side` for its parenthesised variants, so a name
//! needs no shell quoting); every layout lists its channels in WAVE mask order.

use crossfade_core::{Channel, ChannelLayout};

const TABLE: [(&str, u32); 14] = [
    ("mono", 0x4),
    ("stereo", 0x3),
    ("2.1", 0xB),
    ("3.0", 0x7),
    ("3.1", 0xF),
    ("4.0", 0x107),
    ("quad", 0x33),
    ("quad-side", 0x603),
    ("5.0", 0x607),
    ("5.0-back", 0x37),
    ("5.1", 0x60F),
    ("5.1-back", 0x3F),
    ("6.1", 0x70F),
    ("7.1", 0x63F),
];

#[test]
fn every_name_maps_to_its_mask_and_back() {
    for (name, mask) in TABLE {
        let layout = ChannelLayout::from_name(name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(layout.to_mask(), Some(mask), "{name}");
        assert_eq!(layout.name(), Some(name), "{mask:#x}");
        assert_eq!(
            ChannelLayout::from_mask(mask, layout.channels())
                .unwrap()
                .name(),
            Some(name)
        );
    }
    assert_eq!(ChannelLayout::NAMES.len(), TABLE.len());
    assert!(
        ChannelLayout::NAMES
            .iter()
            .zip(TABLE)
            .all(|(n, (t, _))| *n == t)
    );
}

#[test]
fn mono_and_stereo_are_the_existing_constructors() {
    assert_eq!(
        ChannelLayout::from_name("mono"),
        Some(ChannelLayout::mono())
    );
    assert_eq!(
        ChannelLayout::from_name("stereo"),
        Some(ChannelLayout::stereo())
    );
}

#[test]
fn names_are_exact() {
    for bad in [
        "",
        "Stereo",
        "5.1(side)",
        "5.1 ",
        "6",
        "7.1-wide",
        "surround",
    ] {
        assert_eq!(ChannelLayout::from_name(bad), None, "{bad:?}");
    }
}

#[test]
fn layouts_without_a_name_have_none() {
    assert_eq!(ChannelLayout::unpositioned(2).unwrap().name(), None);
    let swapped =
        ChannelLayout::from_positions(vec![Channel::FrontRight, Channel::FrontLeft]).unwrap();
    assert_eq!(swapped.name(), None);
    assert_eq!(
        ChannelLayout::from_positions(vec![Channel::FrontLeft])
            .unwrap()
            .name(),
        None
    );
}

#[test]
fn each_count_has_the_flac_default_layout() {
    let expected = ["mono", "stereo", "3.0", "quad", "5.0", "5.1", "6.1", "7.1"];
    for (n, name) in (1..=8).zip(expected) {
        let layout = ChannelLayout::default_for(n).unwrap();
        assert_eq!(layout.name(), Some(name), "{n}");
        assert_eq!(layout.channels(), n);
    }
    assert_eq!(
        ChannelLayout::default_for(9),
        ChannelLayout::unpositioned(9)
    );
    assert!(ChannelLayout::default_for(0).is_err());
}
