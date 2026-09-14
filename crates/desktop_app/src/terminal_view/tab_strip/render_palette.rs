use super::super::*;

#[derive(Clone, Copy)]
pub(super) struct TabStripPalette {
    pub(super) tab_stroke_color: gpui::Rgba,
    pub(super) inactive_tab_bg: gpui::Rgba,
    pub(super) active_tab_bg: gpui::Rgba,
    pub(super) hovered_tab_bg: gpui::Rgba,
    pub(super) active_tab_text: gpui::Rgba,
    pub(super) inactive_tab_text: gpui::Rgba,
    pub(super) active_tab_indicator: gpui::Rgba,
    pub(super) close_button_bg: gpui::Rgba,
    pub(super) close_button_hover_bg: gpui::Rgba,
    pub(super) close_button_hover_text: gpui::Rgba,
    pub(super) switch_hint_bg: gpui::Rgba,
    pub(super) switch_hint_border: gpui::Rgba,
    pub(super) switch_hint_text: gpui::Rgba,
    pub(super) tab_drop_marker_color: gpui::Rgba,
}

pub(super) fn resolve_branding_text_color(palette: &TabStripPalette) -> gpui::Rgba {
    let mut color = palette.inactive_tab_text;
    color.a = color.a.max(TAB_STRIP_BRANDING_TEXT_ALPHA_FLOOR);
    color
}

impl TerminalView {
    pub(super) fn resolve_tab_strip_palette(
        &self,
        colors: &TerminalColors,
        tabbar_bg: gpui::Rgba,
    ) -> TabStripPalette {
        let tab_stroke_color = resolve_chrome_stroke_color(
            tabbar_bg,
            colors.foreground,
            self.chrome_contrast_profile().stroke_mix,
        );
        let mut inactive_tab_bg = colors.foreground;
        inactive_tab_bg.a = 0.0;
        let mut active_tab_bg = colors.foreground;
        active_tab_bg.a = self.scaled_chrome_surface_alpha(0.11);
        let mut hovered_tab_bg = colors.foreground;
        hovered_tab_bg.a = self.scaled_chrome_surface_alpha(0.05);
        let mut active_tab_text = colors.foreground;
        active_tab_text.a = 0.95;
        let mut inactive_tab_text = colors.foreground;
        inactive_tab_text.a = 0.62;
        let mut active_tab_indicator = colors.cursor;
        active_tab_indicator.a = self.scaled_chrome_accent_alpha(0.90);
        let mut close_button_bg = colors.foreground;
        close_button_bg.a = 0.0;
        let mut close_button_hover_bg = colors.foreground;
        close_button_hover_bg.a = self.scaled_chrome_surface_alpha(0.10);
        let mut close_button_hover_text = colors.foreground;
        close_button_hover_text.a = 1.0;
        let now = Instant::now();
        let hint_progress = self.tab_switch_hint_progress(now);
        let mut switch_hint_bg = colors.cursor;
        switch_hint_bg.a = self.scaled_chrome_accent_alpha(0.18 * hint_progress);
        let mut switch_hint_border = colors.cursor;
        switch_hint_border.a = self.scaled_chrome_accent_alpha(0.52 * hint_progress);
        let mut switch_hint_text = colors.foreground;
        switch_hint_text.a = (0.99 * hint_progress).clamp(0.0, 1.0);
        let mut tab_drop_marker_color = colors.cursor;
        tab_drop_marker_color.a = self.scaled_chrome_accent_alpha(0.95);
        TabStripPalette {
            tab_stroke_color,
            inactive_tab_bg,
            active_tab_bg,
            hovered_tab_bg,
            active_tab_text,
            inactive_tab_text,
            active_tab_indicator,
            close_button_bg,
            close_button_hover_bg,
            close_button_hover_text,
            switch_hint_bg,
            switch_hint_border,
            switch_hint_text,
            tab_drop_marker_color,
        }
    }
}
