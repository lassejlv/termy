use super::super::{
    COMMAND_PALETTE_ICON_TILE_IDLE_ALPHA, COMMAND_PALETTE_ICON_TILE_SELECTED_ALPHA,
    COMMAND_PALETTE_INPUT_SELECTION_ALPHA, COMMAND_PALETTE_MATCH_TEXT_ALPHA,
    COMMAND_PALETTE_PANEL_BG_ALPHA, COMMAND_PALETTE_PANEL_SOLID_ALPHA,
    COMMAND_PALETTE_ROW_SELECTED_BG_ALPHA, COMMAND_PALETTE_SCROLLBAR_THUMB_ALPHA,
    COMMAND_PALETTE_SCROLLBAR_TRACK_ALPHA, COMMAND_PALETTE_SHORTCUT_BG_ALPHA,
    COMMAND_PALETTE_SHORTCUT_TEXT_ALPHA, OVERLAY_MUTED_TEXT_ALPHA, OVERLAY_PRIMARY_TEXT_ALPHA,
    TerminalView, resolve_chrome_stroke_color,
};
use crate::colors::TerminalColors;

pub(in super::super) const COMMAND_PALETTE_PANEL_RADIUS: f32 = 14.0;
pub(super) const COMMAND_PALETTE_ROW_RADIUS: f32 = 8.0;
pub(super) const COMMAND_PALETTE_SHORTCUT_RADIUS: f32 = 6.0;

#[derive(Clone, Copy)]
pub(in super::super) struct CommandPaletteStyle {
    pub(in super::super) panel_bg: gpui::Rgba,
    pub(in super::super) panel_border: gpui::Rgba,
    pub(in super::super) primary_text: gpui::Rgba,
    pub(in super::super) muted_text: gpui::Rgba,
    pub(in super::super) input_selection: gpui::Rgba,
    pub(super) selected_bg: gpui::Rgba,
    // Accent applied to the characters a query matched in a row title.
    pub(super) match_text: gpui::Rgba,
    pub(super) shortcut_bg: gpui::Rgba,
    pub(super) shortcut_text: gpui::Rgba,
    pub(super) scrollbar_track: gpui::Rgba,
    pub(super) scrollbar_thumb: gpui::Rgba,
    icon_tile_alpha_idle: f32,
    icon_tile_alpha_selected: f32,
}

pub(super) fn command_palette_border_color(
    chrome_surface_bg: gpui::Rgba,
    foreground: gpui::Rgba,
    stroke_mix: f32,
) -> gpui::Rgba {
    resolve_chrome_stroke_color(chrome_surface_bg, foreground, stroke_mix)
}

/// Theme ANSI colour that identifies a palette category, matching the
/// colour-coded tiles in Settings.
pub(super) fn category_tint(colors: &TerminalColors, category: &str) -> gpui::Rgba {
    let mut tint = match category {
        "Tabs" => colors.ansi[6],
        "Panes" => colors.cursor,
        "Window" | "App" => colors.foreground,
        "Sessions" | "SSH" => colors.ansi[2],
        "Search" => colors.ansi[5],
        "Edit" | "Plugins" => colors.ansi[3],
        "Appearance" => colors.ansi[4],
        "Settings" => colors.ansi[1],
        "Tasks" => colors.cursor,
        _ => colors.ansi[4],
    };
    tint.a = 1.0;
    tint
}

impl CommandPaletteStyle {
    pub(in super::super) fn resolve(view: &TerminalView) -> Self {
        let overlay_style = view.overlay_style();
        let panel_bg = overlay_style.chrome_panel_background_with_floor(
            COMMAND_PALETTE_PANEL_BG_ALPHA,
            COMMAND_PALETTE_PANEL_SOLID_ALPHA,
        );

        let mut chrome_surface_bg = view.colors.background;
        chrome_surface_bg.a = view.scaled_background_alpha(chrome_surface_bg.a);
        let panel_border = command_palette_border_color(
            chrome_surface_bg,
            view.colors.foreground,
            view.chrome_contrast_profile().stroke_mix,
        );

        let selected_bg = overlay_style.chrome_panel_neutral(COMMAND_PALETTE_ROW_SELECTED_BG_ALPHA);
        let primary_text = overlay_style.panel_foreground(OVERLAY_PRIMARY_TEXT_ALPHA);
        let muted_text = overlay_style.panel_foreground(OVERLAY_MUTED_TEXT_ALPHA);
        let input_selection =
            overlay_style.chrome_panel_cursor(COMMAND_PALETTE_INPUT_SELECTION_ALPHA);
        let match_text = overlay_style.chrome_panel_cursor(COMMAND_PALETTE_MATCH_TEXT_ALPHA);
        let shortcut_bg = overlay_style.chrome_panel_neutral(COMMAND_PALETTE_SHORTCUT_BG_ALPHA);
        let shortcut_text = overlay_style.panel_foreground(COMMAND_PALETTE_SHORTCUT_TEXT_ALPHA);
        let scrollbar_track =
            view.scrollbar_color(overlay_style, COMMAND_PALETTE_SCROLLBAR_TRACK_ALPHA);
        let scrollbar_thumb =
            view.scrollbar_color(overlay_style, COMMAND_PALETTE_SCROLLBAR_THUMB_ALPHA);
        let contrast = view.chrome_contrast_profile();

        Self {
            panel_bg,
            panel_border,
            primary_text,
            muted_text,
            input_selection,
            selected_bg,
            match_text,
            shortcut_bg,
            shortcut_text,
            scrollbar_track,
            scrollbar_thumb,
            icon_tile_alpha_idle: contrast.accent_alpha(COMMAND_PALETTE_ICON_TILE_IDLE_ALPHA),
            icon_tile_alpha_selected: contrast
                .accent_alpha(COMMAND_PALETTE_ICON_TILE_SELECTED_ALPHA),
        }
    }

    pub(super) fn icon_tile_bg(&self, tint: gpui::Rgba, selected: bool) -> gpui::Rgba {
        let mut fill = tint;
        fill.a = if selected {
            self.icon_tile_alpha_selected
        } else {
            self.icon_tile_alpha_idle
        };
        fill
    }

    pub(super) fn icon_tile_glyph(&self, tint: gpui::Rgba, enabled: bool) -> gpui::Rgba {
        if enabled { tint } else { self.muted_text }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::TerminalColors;

    #[test]
    fn rounded_geometry_uses_consistent_radii() {
        assert_eq!(COMMAND_PALETTE_PANEL_RADIUS, 14.0);
        assert_eq!(COMMAND_PALETTE_ROW_RADIUS, 8.0);
        assert_eq!(COMMAND_PALETTE_SHORTCUT_RADIUS, 6.0);
    }

    #[test]
    fn command_palette_border_matches_shared_chrome_stroke_derivation() {
        let chrome_surface_bg = gpui::Rgba {
            r: 0.02,
            g: 0.05,
            b: 0.12,
            a: 0.9,
        };
        let foreground = gpui::Rgba {
            r: 0.8,
            g: 0.88,
            b: 0.93,
            a: 1.0,
        };

        let stroke_mix = crate::chrome_style::ChromeContrastProfile::from_enabled(false).stroke_mix;
        let border = command_palette_border_color(chrome_surface_bg, foreground, stroke_mix);
        let tab_stroke = resolve_chrome_stroke_color(chrome_surface_bg, foreground, stroke_mix);

        assert_eq!(border, tab_stroke);
    }

    #[test]
    fn known_categories_use_distinct_theme_slots() {
        let colors = TerminalColors {
            background: gpui::Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
            foreground: gpui::Rgba {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            cursor: gpui::Rgba {
                r: 0.1,
                g: 0.2,
                b: 0.3,
                a: 1.0,
            },
            ansi: std::array::from_fn(|index| gpui::Rgba {
                r: index as f32 / 16.0,
                g: 0.4,
                b: 0.5,
                a: 1.0,
            }),
        };

        assert_eq!(category_tint(&colors, "Tabs"), {
            let mut tint = colors.ansi[6];
            tint.a = 1.0;
            tint
        });
        assert_eq!(category_tint(&colors, "Panes"), {
            let mut tint = colors.cursor;
            tint.a = 1.0;
            tint
        });
        assert_eq!(category_tint(&colors, "Settings"), {
            let mut tint = colors.ansi[1];
            tint.a = 1.0;
            tint
        });
        assert_eq!(
            category_tint(&colors, "Plugins"),
            category_tint(&colors, "Edit")
        );
    }
}
