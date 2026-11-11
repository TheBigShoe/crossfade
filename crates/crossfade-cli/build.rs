//! Link settings for the `crossfade` binary.

fn main() {
    // `ctrlc` brings in `objc2`, which links Foundation although nothing calls it, and
    // loading it costs a millisecond or two on every run. Leave out libraries no symbol
    // is used from.
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        println!("cargo:rustc-link-arg-bins=-Wl,-dead_strip_dylibs");
    }
}
