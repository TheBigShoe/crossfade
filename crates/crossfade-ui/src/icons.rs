//! The app's icon set: gpui-kit's bundled icons plus the few Lucide icons the chrome
//! adds. Register [`Assets`] with `Application::with_assets`; without an asset source
//! every SVG icon renders as nothing.

use std::borrow::Cow;

use crate::prelude::*;

/// The icons the app draws, kit's set behind them.
#[derive(Clone, Copy, Debug, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, _path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(None)
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon the app names loads from the registered source.
    #[test]
    fn every_named_icon_loads() {
        for icon in [
            IconName::Plus,
            IconName::FolderOpen,
            IconName::Play,
            IconName::Settings,
            IconName::Ellipsis,
            IconName::X,
            IconName::PanelLeft,
            IconName::PanelRight,
            IconName::PanelBottom,
            IconName::Copy,
            IconName::RotateCcw,
            IconName::Check,
            IconName::TriangleAlert,
        ] {
            let path = icon.path();
            let loaded = Assets.load(&path).expect("no load error");
            assert!(loaded.is_some_and(|bytes| !bytes.is_empty()), "{path}");
        }
    }
}
