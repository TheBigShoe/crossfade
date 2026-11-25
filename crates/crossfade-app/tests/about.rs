use crossfade_app::about;

#[test]
fn about_names_the_app_its_version_what_it_does_and_its_licences() {
    assert_eq!(
        about::lines(),
        [
            format!("Crossfade {}", env!("CARGO_PKG_VERSION")),
            "Converts, inspects and normalises audio files, in pure Rust.".to_owned(),
            "MIT or Apache-2.0; the Opus codec is BSD-3-Clause.".to_owned(),
        ]
    );
}
