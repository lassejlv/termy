//! The kit's icon set, embedded so an embedder only has to register [`Assets`].
//!
//! GPUI rasterizes an SVG into an alpha mask and tints it with the element's
//! text color, so the fills in the source files are irrelevant — only coverage
//! and per-shape `opacity` survive. That is what gives the secondary strokes in
//! `tabs` and `general` their lighter weight.

use std::borrow::Cow;

use gpui::{
    App, AssetSource, IntoElement, Pixels, RenderOnce, Result, Rgba, SharedString, Styled, Window,
    svg,
};

use crate::design_system::metrics::SIDEBAR_ICON_SIZE;
use crate::design_system::theme::tokens;

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        /// Every icon the kit ships.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum IconName {
            $($variant),*
        }

        impl IconName {
            /// Path to look this icon up with through an [`AssetSource`].
            pub fn path(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!("icons/", $file, ".svg")),*
                }
            }
        }

        const EMBEDDED_ICONS: &[(&str, &[u8])] = &[
            $((
                concat!("icons/", $file, ".svg"),
                include_bytes!(concat!("assets/icons/", $file, ".svg")) as &[u8],
            )),*
        ];
    };
}

icons! {
    Appearance => "appearance",
    Colors => "colors",
    Themes => "themes",
    Tabs => "tabs",
    Terminal => "terminal",
    Ssh => "ssh",
    Keybindings => "keybindings",
    Plugins => "plugins",
    General => "general",
    Search => "search",
    ChevronDown => "chevron-down",
    ChevronUp => "chevron-up",
    Check => "check",
    Plus => "plus",
    Minus => "minus",
    Reset => "reset",
    Alert => "alert",
    Close => "close",
    ExternalLink => "external-link",
    Folder => "folder",
}

/// Asset source for the kit's icons.
///
/// Hosts that already have their own [`AssetSource`] should delegate to
/// [`icon_bytes`] for paths under `icons/` rather than registering this.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(icon_bytes(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(EMBEDDED_ICONS
            .iter()
            .filter(|(icon_path, _)| icon_path.starts_with(path))
            .map(|(icon_path, _)| SharedString::from(*icon_path))
            .collect())
    }
}

/// Raw bytes for an embedded icon path, for hosts with their own asset source.
pub fn icon_bytes(path: &str) -> Option<&'static [u8]> {
    EMBEDDED_ICONS
        .iter()
        .find(|(icon_path, _)| *icon_path == path)
        .map(|(_, bytes)| *bytes)
}

/// A tinted icon. Defaults to the sidebar size and muted text color.
#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: Pixels,
    color: Option<Rgba>,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: SIDEBAR_ICON_SIZE,
            color: None,
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or_else(|| tokens(cx).text_secondary);

        svg()
            .path(self.name.path())
            .size(self.size)
            .flex_none()
            .text_color(color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_resolves_to_embedded_bytes() {
        let names = [
            IconName::Appearance,
            IconName::Colors,
            IconName::Themes,
            IconName::Tabs,
            IconName::Terminal,
            IconName::Ssh,
            IconName::Keybindings,
            IconName::Plugins,
            IconName::General,
            IconName::Search,
            IconName::ChevronDown,
            IconName::ChevronUp,
            IconName::Check,
            IconName::Plus,
            IconName::Minus,
            IconName::Reset,
            IconName::Alert,
            IconName::Close,
            IconName::ExternalLink,
            IconName::Folder,
        ];

        assert_eq!(names.len(), EMBEDDED_ICONS.len());
        for name in names {
            let bytes = icon_bytes(name.path())
                .unwrap_or_else(|| panic!("missing asset for {}", name.path()));
            assert!(bytes.starts_with(b"<svg"), "{} is not an svg", name.path());
        }
    }

    #[test]
    fn unknown_paths_are_misses_rather_than_errors() {
        assert!(icon_bytes("icons/nope.svg").is_none());
        assert_eq!(Assets.load("icons/nope.svg").unwrap(), None);
        assert_eq!(Assets.list("icons/").unwrap().len(), EMBEDDED_ICONS.len());
    }
}
