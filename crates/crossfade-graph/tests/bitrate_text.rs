//! `--bitrate` words: a number of bits per second, written plainly or with k, kbps or kb/s.
use crossfade_graph::filter::parse_bitrate;

#[test]
fn these_are_bitrates() {
    for (text, bits) in [
        ("128k", 128_000),
        ("96000", 96_000),
        ("96000bps", 96_000),
        ("64kbps", 64_000),
        ("64kb/s", 64_000),
        ("1.5k", 1_500),
        ("12.345k", 12_345),
        ("510k", 510_000),
    ] {
        assert_eq!(parse_bitrate(text).unwrap(), bits, "{text}");
    }
}

#[test]
fn these_are_not_and_the_message_shows_the_form() {
    for text in [
        "",
        "k",
        "-128k",
        "128K",
        "128 k",
        "128kHz",
        "fast",
        "1e5",
        "12.3456k",
        "128.k",
        "0",
        "0k",
        "5000000000",
    ] {
        let e = parse_bitrate(text).unwrap_err().to_string();
        assert!(
            e.contains("expected a bitrate such as 128k, 96000 or 64kbps")
                || e.contains("at least"),
            "{text:?}: {e}"
        );
    }
}
