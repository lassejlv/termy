use super::*;

impl SettingsWindow {
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

    fn scaled_chrome_surface_alpha(&self, base_alpha: f32) -> f32 {
        self.scaled_background_alpha(self.chrome_contrast_profile().surface_alpha(base_alpha))
    }

    fn scaled_chrome_neutral_alpha(&self, base_alpha: f32) -> f32 {
        self.scaled_background_alpha(
            self.chrome_contrast_profile()
                .panel_neutral_alpha(base_alpha),
        )
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

    pub(super) fn bg_primary(&self) -> Rgba {
        let mut c = self.colors.background;
        c.a = self.scaled_background_alpha(c.a);
        c
    }

    pub(super) fn bg_secondary(&self) -> Rgba {
        let mut c = self.colors.background;
        c.a = self.adaptive_chrome_panel_alpha(0.7);
        c
    }

    pub(super) fn bg_card(&self) -> Rgba {
        let mut c = self.colors.background;
        c.a = self.adaptive_chrome_panel_alpha(0.5);
        c
    }

    pub(super) fn icon_color(&self, active: bool) -> Rgba {
        if active {
            self.accent()
        } else {
            self.text_secondary()
        }
    }

    pub(super) fn bg_elevated(&self) -> Rgba {
        let mut c = self.colors.foreground;
        c.a = self.scaled_chrome_surface_alpha(0.045);
        c
    }

    pub(super) fn divider_color(&self) -> Rgba {
        let mut c = self.colors.foreground;
        c.a = self.scaled_chrome_neutral_alpha(0.10);
        c
    }

    /// Soft outline for grouped setting cards — lighter than `border_color` so
    /// cards read as a gentle inset surface rather than a boxed-in panel.
    pub(super) fn card_border_color(&self) -> Rgba {
        let mut c = self.colors.foreground;
        c.a = self.scaled_chrome_neutral_alpha(0.14);
        c
    }

    /// Hairline between rows inside a card. Subtler than the card outline so the
    /// inset separators recede the way native macOS grouped lists do.
    pub(super) fn row_separator_color(&self) -> Rgba {
        let mut c = self.colors.foreground;
        c.a = self.scaled_chrome_neutral_alpha(0.08);
        c
    }

    /// Accent-tinted fill behind the selected sidebar item.
    pub(super) fn sidebar_selection_bg(&self) -> Rgba {
        self.accent_with_alpha(0.16)
    }

    pub(super) fn bg_input(&self) -> Rgba {
        let mut c = self.colors.background;
        c.a = self.adaptive_chrome_panel_alpha(0.36);
        c
    }

    pub(super) fn bg_hover(&self) -> Rgba {
        let mut c = self.colors.foreground;
        c.a = self.scaled_chrome_surface_alpha(0.1);
        c
    }

    pub(super) fn text_primary(&self) -> Rgba {
        self.colors.foreground
    }

    pub(super) fn text_secondary(&self) -> Rgba {
        let mut c = self.colors.foreground;
        c.a = 0.82;
        c
    }

    pub(super) fn text_muted(&self) -> Rgba {
        let mut c = self.colors.foreground;
        c.a = 0.68;
        c
    }

    pub(super) fn border_color(&self) -> Rgba {
        let mut c = self.colors.foreground;
        c.a = self.scaled_chrome_neutral_alpha(0.24);
        c
    }

    pub(super) fn accent(&self) -> Rgba {
        self.colors.cursor
    }

    pub(super) fn accent_with_alpha(&self, alpha: f32) -> Rgba {
        let mut c = self.colors.cursor;
        c.a = self.scaled_chrome_accent_alpha(alpha);
        c
    }

    /// Projects this window's live chrome colors onto the shared design-system
    /// tokens.
    ///
    /// Every value comes from the helpers above rather than from
    /// `termy_ui::Tokens::from_palette`, because those helpers already fold in
    /// background opacity, the opacity preview, and the chrome-contrast profile.
    /// Deriving tokens from the raw palette instead would paint opaque surfaces
    /// and quietly drop the window's translucency.
    pub(super) fn ui_tokens(&self) -> termy_ui::Tokens {
        termy_ui::Tokens {
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
        if cx.try_global::<termy_ui::Tokens>() != Some(&tokens) {
            termy_ui::set_tokens(tokens, cx);
        }
    }

    pub(super) fn settings_scrollbar_style(&self) -> ScrollbarPaintStyle {
        let mut track = self.colors.foreground;
        track.a = self.scaled_chrome_neutral_alpha(SETTINGS_SCROLLBAR_TRACK_ALPHA);

        let mut thumb = self.colors.foreground;
        thumb.a = self.scaled_chrome_neutral_alpha(SETTINGS_SCROLLBAR_THUMB_ALPHA);

        let mut active_thumb = self.colors.foreground;
        active_thumb.a = self.scaled_chrome_neutral_alpha(SETTINGS_SCROLLBAR_THUMB_ACTIVE_ALPHA);

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
        let max_offset: f32 = self.content_scroll_handle.max_offset().height.into();
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
