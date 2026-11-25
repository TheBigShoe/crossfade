//! "About Crossfade" as JSON: the app layer's lines and the versions.

use serde::Serialize;

#[derive(Serialize)]
struct About {
    lines: Vec<String>,
    version: &'static str,
    abi_version: u32,
}

pub(crate) fn json(abi_version: u32) -> String {
    let about = About {
        lines: crossfade_app::about::lines().into(),
        version: env!("CARGO_PKG_VERSION"),
        abi_version,
    };
    serde_json::to_string(&about).unwrap_or_default()
}
