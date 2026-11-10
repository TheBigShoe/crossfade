//! The app's icon set: gpui-kit's bundled icons plus the few Lucide icons the chrome
//! adds. `KitAssets` embeds only what kit's own components use; [`AppIcons`] adds ours
//! (a few hundred bytes each) in front of it. Register [`Assets`] with
//! `Application::with_assets`; without an asset source every SVG icon renders as nothing.

use std::borrow::Cow;

use crate::prelude::*;

icon_assets!(pub AppIcons, [
    Check,
    BrushCleaning,
    Copy,
    Ellipsis,
    FileMusic,
    FolderOpen,
    PanelBottom,
    PanelLeft,
    PanelRight,
    Play,
    Plus,
    RotateCcw,
    Settings,
    CircleStop,
    TriangleAlert,
    X,
]);

/// The icons the app draws: [`AppIcons`], then gpui-kit's bundled set.
#[derive(Clone, Copy, Debug, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match AppIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => KitAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = AppIcons.list(path)?;
        paths.extend(KitAssets.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
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
            IconName::ArrowRightLeft,
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
            IconName::FileMusic,
            IconName::CircleStop,
            IconName::BrushCleaning,
        ] {
            let path = icon.path();
            let loaded = Assets.load(&path).expect("no load error");
            assert!(loaded.is_some_and(|bytes| !bytes.is_empty()), "{path}");
        }
    }
}
