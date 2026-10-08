use super::*;
use crate::ui::native::{NativePalette, tile_gradient, with_alpha};
use gpui_kit::rgba;

impl SettingsWindow {
    /// AppKit semantic colors for this window, picked from the theme ground
    /// so light themes get the light appearance.
    pub(super) fn native(&self) -> NativePalette {
        NativePalette::for_background(self.colors.background)
            .with_increased_contrast(self.config.chrome_contrast)
    }

    /// The opaque surface under the material, as `windowBackgroundColor`.
    fn native_surface(&self) -> Rgba {
        if self.native().dark {
            rgba(0x1e1e1eff)
        } else {
            rgba(0xececec_ff)
        }
    }

    pub(super) fn background_opacity_factor(&self) -> f32 {
        self.effective_background_opacity()
    }

    pub(super) fn scaled_background_alpha(&self, base_alpha: f32) -> f32 {
        (base_alpha * self.background_opacity_factor()).clamp(0.0, 1.0)
    }

    pub(super) fn chrome_contrast_profile(&self) -> crate::chrome_style::ChromeContrastProfile {
        crate::chrome_style::ChromeContrastProfile::from_enabled(self.config.chrome_contrast)
    }

    fn adaptive_chrome_panel_alpha(&self, base_alpha: f32) -> f32 {
        let profile = self.chrome_contrast_profile();
        let scaled_alpha = profile.panel_surface_alpha(base_alpha);
        let floor = scaled_alpha * SETTINGS_OVERLAY_PANEL_ALPHA_FLOOR_RATIO;
        self.scaled_background_alpha(scaled_alpha)
            .max(floor)
            .clamp(0.0, 1.0)
    }

    fn scaled_chrome_accent_alpha(&self, base_alpha: f32) -> f32 {
        self.scaled_background_alpha(
            self.chrome_contrast_profile()
                .panel_accent_alpha(base_alpha),
        )
    }

    pub(super) fn sync_window_background_appearance(&mut self, window: &mut Window) {
        let mut preview_config = self.config.clone();
        preview_config.background_opacity = self.effective_background_opacity();
        let appearance =
            crate::terminal_view::initial_window_background_appearance(&preview_config);
        if self.last_window_background_appearance != Some(appearance) {
            window.set_background_appearance(appearance);
            self.last_window_background_appearance = Some(appearance);
        }
    }

    /// The content pane, `windowBackgroundColor` under the window's opacity.
    pub(super) fn bg_primary(&self) -> Rgba {
        let mut c = self.native_surface();
        c.a = self.scaled_background_alpha(1.0);
        c
    }

    /// The sidebar, a step darker than the content as in System Settings.
    pub(super) fn bg_secondary(&self) -> Rgba {
        let native = self.native();
        let overlay = if native.dark {
            rgba(0x00000033)
        } else {
            rgba(0x0000000a)
        };
        let mut c = Self::composite_over(overlay, self.native_surface());
        c.a = self.adaptive_chrome_panel_alpha(0.92);
        c
    }

    pub(super) fn bg_card(&self) -> Rgba {
        self.bg_primary()
    }

    /// A grouped form section, `quaternarySystemFill` over the window.
    pub(super) fn bg_elevated(&self) -> Rgba {
        self.native().group
    }

    /// The hairline around a grouped section.
    pub(super) fn card_border_color(&self) -> Rgba {
        self.native().separator
    }

    /// Hairline between rows inside a grouped section.
    pub(super) fn row_separator_color(&self) -> Rgba {
        self.native().separator
    }

    /// The selected sidebar row, filled with the accent as in System Settings.
    pub(super) fn sidebar_selection_bg(&self) -> Rgba {
        self.native().blue
    }

    /// `textBackgroundColor` for fields sitting inside a grouped section.
    /// Opaque, so a focus ring drawn around a field never tints through it.
    pub(super) fn bg_input(&self) -> Rgba {
        let native = self.native();
        if native.dark {
            let group = Self::composite_over(native.group, self.native_surface());
            Self::composite_over(rgba(0xffffff0d), group)
        } else {
            rgba(0xffffffff)
        }
    }

    pub(super) fn bg_hover(&self) -> Rgba {
        self.native().fill
    }

    pub(super) fn text_primary(&self) -> Rgba {
        self.native().label
    }

    /// Body text one step below the label, for unselected sidebar rows.
    pub(super) fn text_secondary(&self) -> Rgba {
        let native = self.native();
        with_alpha(native.label, native.label.a * 0.86)
    }

    pub(super) fn text_muted(&self) -> Rgba {
        self.native().secondary
    }

    pub(super) fn border_color(&self) -> Rgba {
        self.native().stroke
    }

    pub(super) fn accent(&self) -> Rgba {
        self.native().blue
    }

    pub(super) fn accent_with_alpha(&self, alpha: f32) -> Rgba {
        with_alpha(self.native().blue, self.scaled_chrome_accent_alpha(alpha))
    }

    /// The focus ring macOS draws around a focused text field or search box.
    pub(super) fn input_focus_ring(&self) -> Rgba {
        with_alpha(self.native().blue, 0.5)
    }

    /// Zero-blur spread shadow that reads as a focus ring around a control.
    pub(super) fn focus_ring_shadow(color: Rgba) -> gpui_kit::BoxShadow {
        gpui_kit::BoxShadow {
            inset: false,
            color: color.into(),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(SETTINGS_INPUT_FOCUS_RING_WIDTH),
        }
    }

    /// Colour that identifies a section across the sidebar and its header.
    /// Pulled from the active theme's ANSI palette so every theme keeps the
    /// tiles readable; the two neutral sections use the foreground instead.
    pub(super) fn section_tint(&self, section: SettingsSection) -> Rgba {
        let native = self.native();
        match section {
            SettingsSection::Advanced | SettingsSection::Keybindings => native.gray,
            SettingsSection::Appearance => native.blue,
            SettingsSection::Colors => native.red,
            SettingsSection::ThemeStore => native.purple,
            SettingsSection::Plugins => native.orange,
            SettingsSection::Terminal => rgba(0x3a3a3cff),
            SettingsSection::Ssh => native.green,
            SettingsSection::Tabs => rgba(0x5ac8faff),
        }
    }

    /// Tiles are solid system colors with a white glyph, like the rows in
    /// System Settings.
    pub(super) fn section_tile_bg(&self, section: SettingsSection, _emphasized: bool) -> Rgba {
        self.section_tint(section)
    }

    pub(super) fn section_tile_icon(&self, _section: SettingsSection) -> Rgba {
        self.native().on_accent
    }

    /// Rounded, tinted square holding a section glyph.
    pub(super) fn render_section_tile(
        &self,
        section: SettingsSection,
        tile_size: f32,
        tile_radius: f32,
        icon_size: f32,
        emphasized: bool,
    ) -> gpui_kit::Div {
        div()
            .flex_none()
            .w(px(tile_size))
            .h(px(tile_size))
            .rounded(px(tile_radius))
            .bg(tile_gradient(self.section_tile_bg(section, emphasized)))
            .shadow(vec![gpui_kit::BoxShadow {
                inset: false,
                color: gpui_kit::black().opacity(0.18),
                offset: point(px(0.0), px(0.5)),
                blur_radius: px(1.0),
                spread_radius: px(0.0),
            }])
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(SharedString::from(Self::section_icon_path(section)))
                    .size(px(icon_size))
                    .text_color(self.section_tile_icon(section)),
            )
    }

    /// Projects this window's live chrome colors onto the shared design-system
    /// tokens.
    ///
    /// Every value comes from the helpers above rather than from
    /// `crate::design_system::Tokens::from_palette`, because those helpers already fold in
    /// background opacity, the opacity preview, and the chrome-contrast profile.
    /// Deriving tokens from the raw palette instead would paint opaque surfaces
    /// and quietly drop the window's translucency.
    pub(super) fn ui_tokens(&self) -> crate::design_system::Tokens {
        crate::design_system::Tokens {
            bg_window: self.bg_primary(),
            bg_panel: self.bg_secondary(),
            // Cards in this window ride the elevated surface, not `bg_card()`,
            // which is the panel fill behind them.
            bg_card: self.bg_elevated(),
            bg_input: self.bg_input(),
            bg_hover: self.bg_hover(),
            bg_overlay: self.bg_elevated(),

            border: self.border_color(),
            card_border: self.card_border_color(),
            row_separator: self.row_separator_color(),

            text_primary: self.text_primary(),
            text_secondary: self.text_secondary(),
            text_muted: self.text_muted(),
            text_on_accent: self.contrasting_text_for_fill(self.accent(), self.bg_card()),

            accent: self.accent(),
            accent_soft: self.sidebar_selection_bg(),

            success: self.colors.ansi[2],
            warning: self.colors.ansi[3],
            danger: self.colors.ansi[1],
        }
    }

    /// Publishes the tokens the kit's components read, skipping the write when
    /// nothing changed so a repaint does not churn the global.
    pub(super) fn sync_ui_tokens(&self, cx: &mut Context<Self>) {
        let tokens = self.ui_tokens();
        if cx.try_global::<crate::design_system::Tokens>() != Some(&tokens) {
            crate::design_system::set_tokens(tokens, cx);
        }
    }

    pub(super) fn settings_scrollbar_style(&self) -> ScrollbarPaintStyle {
        let label = self.native().label;
        let track = with_alpha(label, SETTINGS_SCROLLBAR_TRACK_ALPHA * 0.5);
        let thumb = with_alpha(label, SETTINGS_SCROLLBAR_THUMB_ALPHA);
        let active_thumb = with_alpha(label, SETTINGS_SCROLLBAR_THUMB_ACTIVE_ALPHA);

        ScrollbarPaintStyle {
            width: SETTINGS_SCROLLBAR_WIDTH,
            track_radius: 4.0,
            thumb_radius: 4.0,
            thumb_inset: 1.0,
            marker_inset: 0.0,
            marker_radius: 0.0,
            track_color: track,
            thumb_color: thumb,
            active_thumb_color: active_thumb,
            marker_color: None,
            current_marker_color: None,
        }
    }

    pub(super) fn settings_scrollbar_range(&self, window: &Window) -> ScrollbarRange {
        let viewport_height: f32 = window.viewport_size().height.into();
        let max_offset: f32 = self.content_scroll_handle.max_offset().y.into();
        let offset_y: f32 = self.content_scroll_handle.offset().y.into();
        let offset = (-offset_y).max(0.0);
        ScrollbarRange {
            offset,
            max_offset,
            viewport_extent: viewport_height,
            track_extent: viewport_height,
        }
    }

    pub(super) fn settings_scrollbar_metrics(
        &self,
        window: &Window,
    ) -> Option<ui_scrollbar::ScrollbarMetrics> {
        ui_scrollbar::compute_metrics(
            self.settings_scrollbar_range(window),
            SETTINGS_SCROLLBAR_MIN_THUMB_HEIGHT,
        )
    }

    pub(super) fn apply_scrollbar_offset(&mut self, offset: f32, max_offset: f32) {
        let clamped = offset.clamp(0.0, max_offset);
        self.content_scroll_handle
            .set_offset(point(px(0.0), px(-clamped)));
    }

    pub(super) fn handle_scrollbar_mouse_down(
        &mut self,
        window_y: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.scrollbar_lane_bounds else {
            return;
        };
        let lane_top: f32 = bounds.top().into();
        let local_y = window_y - lane_top;
        let range = self.settings_scrollbar_range(window);
        let Some(metrics) =
            ui_scrollbar::compute_metrics(range, SETTINGS_SCROLLBAR_MIN_THUMB_HEIGHT)
        else {
            return;
        };
        let thumb_top = metrics.thumb_top;
        let thumb_bottom = thumb_top + metrics.thumb_height;
        if local_y >= thumb_top && local_y <= thumb_bottom {
            self.scrollbar_drag_state = Some(ScrollbarDragState {
                thumb_grab_offset: local_y - thumb_top,
            });
        } else {
            let new_offset = ui_scrollbar::offset_from_track_click(local_y, range, metrics);
            self.apply_scrollbar_offset(new_offset, range.max_offset);
            self.scrollbar_drag_state = Some(ScrollbarDragState {
                thumb_grab_offset: metrics.thumb_height * 0.5,
            });
        }
        cx.notify();
    }

    pub(super) fn handle_scrollbar_drag(
        &mut self,
        window_y: f32,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.scrollbar_drag_state else {
            return;
        };
        let Some(bounds) = self.scrollbar_lane_bounds else {
            return;
        };
        let lane_top: f32 = bounds.top().into();
        let local_y = window_y - lane_top;
        let range = self.settings_scrollbar_range(window);
        let Some(metrics) =
            ui_scrollbar::compute_metrics(range, SETTINGS_SCROLLBAR_MIN_THUMB_HEIGHT)
        else {
            return;
        };
        let target_thumb_top = (local_y - drag.thumb_grab_offset).clamp(0.0, metrics.travel);
        let new_offset = ui_scrollbar::offset_from_thumb_top(target_thumb_top, range, metrics);
        self.apply_scrollbar_offset(new_offset, range.max_offset);
        cx.notify();
    }

    pub(super) fn finish_scrollbar_drag(&mut self) -> bool {
        self.scrollbar_drag_state.take().is_some()
    }

    pub(super) fn request_scrollbar_refresh_frames(
        &mut self,
        frames_remaining: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if frames_remaining == 0 {
            return;
        }

        let this = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let _ = this.update(cx, |view, cx| {
                cx.notify();
                view.request_scrollbar_refresh_frames(frames_remaining - 1, window, cx);
            });
        });
    }

    pub(super) fn srgb_to_linear(channel: f32) -> f32 {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    pub(super) fn composite_over(fg: Rgba, bg: Rgba) -> Rgba {
        let fg_alpha = fg.a.clamp(0.0, 1.0);
        Rgba {
            r: (fg_alpha * fg.r + (1.0 - fg_alpha) * bg.r).clamp(0.0, 1.0),
            g: (fg_alpha * fg.g + (1.0 - fg_alpha) * bg.g).clamp(0.0, 1.0),
            b: (fg_alpha * fg.b + (1.0 - fg_alpha) * bg.b).clamp(0.0, 1.0),
            a: 1.0,
        }
    }

    pub(super) fn relative_luminance(color: Rgba, backdrop: Rgba) -> f32 {
        let composited = Self::composite_over(color, backdrop);
        let r = Self::srgb_to_linear(composited.r);
        let g = Self::srgb_to_linear(composited.g);
        let b = Self::srgb_to_linear(composited.b);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    pub(super) fn contrast_ratio(a: Rgba, b: Rgba, backdrop: Rgba) -> f32 {
        let l1 = Self::relative_luminance(a, backdrop);
        let l2 = Self::relative_luminance(b, backdrop);
        let (lighter, darker) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
        (lighter + 0.05) / (darker + 0.05)
    }

    pub(super) fn contrasting_text_for_fill(&self, fill: Rgba, backdrop: Rgba) -> Rgba {
        let mut primary = self.text_primary();
        primary.a = 1.0;
        let mut dark = self.bg_primary();
        dark.a = 1.0;
        let mut backdrop = backdrop;
        backdrop.a = 1.0;
        let composited_fill = Self::composite_over(fill, backdrop);

        if Self::contrast_ratio(primary, composited_fill, backdrop)
            >= Self::contrast_ratio(dark, composited_fill, backdrop)
        {
            primary
        } else {
            dark
        }
    }
}
