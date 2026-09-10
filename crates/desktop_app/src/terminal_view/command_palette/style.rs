use super::super::TerminalView;
use gpui::Rgba;
use termy_ui::theme::with_alpha;

pub(in super::super) const COMMAND_PALETTE_PANEL_RADIUS: f32 = 12.0;
pub(super) const COMMAND_PALETTE_ROW_RADIUS: f32 = 6.0;
pub(super) const COMMAND_PALETTE_SHORTCUT_RADIUS: f32 = 5.0;

#[derive(Clone, Copy)]
pub(in super::super) struct CommandPaletteStyle {
    pub(in super::super) panel_bg: Rgba,
    pub(in super::super) panel_border: Rgba,
    pub(in super::super) primary_text: Rgba,
    pub(in super::super) muted_text: Rgba,
    pub(in super::super) input_selection: Rgba,
    pub(super) selected_bg: Rgba,
    pub(super) selected_text: Rgba,
    pub(super) shortcut_bg: Rgba,
    pub(super) shortcut_text: Rgba,
    pub(super) divider: Rgba,
    pub(super) footer_bg: Rgba,
    pub(super) scrollbar_track: Rgba,
    pub(super) scrollbar_thumb: Rgba,
}

impl CommandPaletteStyle {
    pub(in super::super) fn resolve(view: &TerminalView) -> Self {
        Self::from_palette(termy_ui::Palette {
            background: view.colors.background,
            foreground: view.colors.foreground,
            cursor: view.colors.cursor,
            green: view.colors.ansi[2],
            yellow: view.colors.ansi[3],
            red: view.colors.ansi[1],
        })
    }

    fn from_palette(palette: termy_ui::Palette) -> Self {
        // Use the same theme and contrast rules as Settings. The floating
        // surface stays opaque so terminal output cannot interfere with labels.
        let tokens = termy_ui::Tokens::for_settings(palette);
        Self {
            panel_bg: tokens.bg_panel,
            panel_border: tokens.border,
            primary_text: tokens.text_primary,
            muted_text: tokens.text_muted,
            input_selection: with_alpha(tokens.accent, 0.28),
            selected_bg: tokens.accent,
            selected_text: tokens.text_on_accent,
            shortcut_bg: tokens.bg_card,
            shortcut_text: tokens.text_secondary,
            divider: tokens.row_separator,
            footer_bg: tokens.bg_input,
            scrollbar_track: with_alpha(tokens.text_primary, 0.04),
            scrollbar_thumb: with_alpha(tokens.text_primary, 0.30),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_uses_settings_theme_and_selection_contrast_in_both_appearances() {
        for palette in [
            termy_ui::Palette::default(),
            termy_ui::Palette {
                background: gpui::rgb(0xfafafa),
                foreground: gpui::rgb(0x383a42),
                cursor: gpui::rgb(0x006cde),
                ..Default::default()
            },
        ] {
            let style = CommandPaletteStyle::from_palette(palette);
            let settings = termy_ui::Tokens::for_settings(palette);
            assert_eq!(style.primary_text, settings.text_primary);
            assert_eq!(style.selected_bg, palette.cursor);
            assert_eq!(style.selected_text, settings.text_on_accent);
            assert_eq!(style.panel_bg.a, 1.0);
            assert_ne!(style.selected_bg, style.selected_text);
        }
    }
}
