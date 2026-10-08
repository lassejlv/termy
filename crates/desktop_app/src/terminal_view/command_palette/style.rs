use super::super::{
    COMMAND_PALETTE_PANEL_BG_ALPHA, COMMAND_PALETTE_PANEL_SOLID_ALPHA,
    COMMAND_PALETTE_SCROLLBAR_THUMB_ALPHA, COMMAND_PALETTE_SCROLLBAR_TRACK_ALPHA, TerminalView,
};
use crate::colors::TerminalColors;
use crate::ui::native::{NativePalette, with_alpha};

/// A popover's corner, as `NSPopover` and Spotlight draw it.
pub(in super::super) const COMMAND_PALETTE_PANEL_RADIUS: f32 = 16.0;
pub(super) const COMMAND_PALETTE_ROW_RADIUS: f32 = 8.0;
pub(super) const COMMAND_PALETTE_SHORTCUT_RADIUS: f32 = 6.0;

#[derive(Clone, Copy)]
pub(in super::super) struct CommandPaletteStyle {
    pub(in super::super) native: NativePalette,
    pub(in super::super) panel_bg: gpui_kit::Rgba,
    pub(in super::super) panel_border: gpui_kit::Rgba,
    pub(in super::super) primary_text: gpui_kit::Rgba,
    pub(in super::super) muted_text: gpui_kit::Rgba,
    pub(in super::super) input_selection: gpui_kit::Rgba,
    pub(super) selected_bg: gpui_kit::Rgba,
    // Accent applied to the characters a query matched in a row title.
    pub(super) match_text: gpui_kit::Rgba,
    pub(super) shortcut_bg: gpui_kit::Rgba,
    pub(super) shortcut_text: gpui_kit::Rgba,
    pub(super) scrollbar_track: gpui_kit::Rgba,
    pub(super) scrollbar_thumb: gpui_kit::Rgba,
    icon_tile_alpha_idle: f32,
    icon_tile_alpha_selected: f32,
}

/// System color that identifies a palette category, matching the tiles in
/// Settings: icons sit on solid tiles like System Settings rows.
pub(super) fn category_tint(colors: &TerminalColors, category: &str) -> gpui_kit::Rgba {
    let native = NativePalette::for_background(colors.background);
    match category {
        "Tabs" | "Panes" | "Window" => native.blue,
        "App" => native.gray,
        "Sessions" | "SSH" => native.green,
        "Search" => native.gray,
        "Edit" | "Plugins" => native.orange,
        "Appearance" => native.purple,
        "Settings" => native.gray,
        "Tasks" => native.red,
        _ => native.purple,
    }
}

impl CommandPaletteStyle {
    pub(in super::super) fn resolve(view: &TerminalView) -> Self {
        let native = NativePalette::for_background(view.colors.background)
            .with_increased_contrast(view.chrome_contrast_profile().stroke_mix > 0.12);
        let overlay_style = view.overlay_style();
        // The popover material: a neutral surface, never the theme ground,
        // so the palette reads as system UI over any theme.
        let mut panel_bg = if native.dark {
            gpui_kit::rgba(0x2a2a2dff)
        } else {
            gpui_kit::rgba(0xf6f6f6ff)
        };
        panel_bg.a = overlay_style
            .chrome_panel_background_with_floor(
                COMMAND_PALETTE_PANEL_BG_ALPHA,
                COMMAND_PALETTE_PANEL_SOLID_ALPHA,
            )
            .a
            .max(0.94);
        let panel_border = native.stroke;
        let scrollbar_track = view.scrollbar_color(overlay_style, COMMAND_PALETTE_SCROLLBAR_TRACK_ALPHA);
        let scrollbar_thumb = view.scrollbar_color(overlay_style, COMMAND_PALETTE_SCROLLBAR_THUMB_ALPHA);

        Self {
            native,
            panel_bg,
            panel_border,
            primary_text: native.label,
            muted_text: native.secondary,
            input_selection: with_alpha(native.blue, 0.35),
            selected_bg: native.selection(),
            match_text: native.label,
            shortcut_bg: native.keycap,
            shortcut_text: native.secondary,
            scrollbar_track,
            scrollbar_thumb,
            icon_tile_alpha_idle: 1.0,
            icon_tile_alpha_selected: 0.22,
        }
    }

    /// The tile behind a row's glyph: a solid system color, or a translucent
    /// white chip on the selected row so it sits on the accent fill.
    pub(super) fn icon_tile_fill(&self, tint: gpui_kit::Rgba, selected: bool) -> gpui_kit::Background {
        if selected {
            with_alpha(gpui_kit::rgba(0xffffffff), self.icon_tile_alpha_selected).into()
        } else {
            crate::ui::native::tile_gradient(with_alpha(tint, self.icon_tile_alpha_idle))
        }
    }

    /// The glyph on an icon tile: white on color, muted when disabled.
    pub(super) fn icon_tile_glyph(&self, enabled: bool) -> gpui_kit::Rgba {
        if enabled {
            self.native.on_accent
        } else {
            self.muted_text
        }
    }

    /// Text and glyph color on a selected (accent-filled) row.
    pub(super) fn selected_text(&self) -> gpui_kit::Rgba {
        self.native.on_accent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::TerminalColors;

    #[test]
    fn rounded_geometry_uses_consistent_radii() {
        assert_eq!(COMMAND_PALETTE_PANEL_RADIUS, 16.0);
        assert_eq!(COMMAND_PALETTE_ROW_RADIUS, 8.0);
        assert_eq!(COMMAND_PALETTE_SHORTCUT_RADIUS, 6.0);
    }

    fn colors_on(background: gpui_kit::Rgba) -> TerminalColors {
        TerminalColors {
            background,
            foreground: gpui_kit::rgba(0xffffffff),
            cursor: gpui_kit::rgba(0x336699ff),
            ansi: std::array::from_fn(|_| gpui_kit::rgba(0x808080ff)),
        }
    }

    #[test]
    fn categories_use_system_colors_not_theme_slots() {
        let colors = colors_on(gpui_kit::rgba(0x000000ff));
        let native = NativePalette::DARK;
        assert_eq!(category_tint(&colors, "Tabs"), native.blue);
        assert_eq!(category_tint(&colors, "Sessions"), native.green);
        assert_eq!(category_tint(&colors, "Plugins"), native.orange);
        assert_eq!(
            category_tint(&colors, "Plugins"),
            category_tint(&colors, "Edit")
        );
    }

    #[test]
    fn categories_follow_the_light_appearance_on_light_themes() {
        let colors = colors_on(gpui_kit::rgba(0xfafafaff));
        assert_eq!(category_tint(&colors, "Tabs"), NativePalette::LIGHT.blue);
    }
}
