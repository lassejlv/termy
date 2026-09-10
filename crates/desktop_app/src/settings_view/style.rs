use super::*;

impl SettingsWindow {
    pub(super) fn sync_window_background_appearance(&mut self, window: &mut Window) {
        // Terminal opacity still previews in terminal windows. Settings always
        // has a solid ground, including when reduced transparency is enabled.
        let appearance = WindowBackgroundAppearance::Opaque;
        if self.last_window_background_appearance != Some(appearance) {
            window.set_background_appearance(appearance);
            self.last_window_background_appearance = Some(appearance);
        }
    }

    pub(super) fn ui_tokens(&self) -> termy_ui::Tokens {
        termy_ui::Tokens::for_settings(termy_ui::Palette {
            background: self.colors.background,
            foreground: self.colors.foreground,
            cursor: self.colors.cursor,
            green: self.colors.ansi[2],
            yellow: self.colors.ansi[3],
            red: self.colors.ansi[1],
        })
    }

    pub(super) fn bg_primary(&self) -> Rgba {
        self.ui_tokens().bg_window
    }

    pub(super) fn bg_secondary(&self) -> Rgba {
        self.ui_tokens().bg_panel
    }

    pub(super) fn bg_card(&self) -> Rgba {
        self.ui_tokens().bg_card
    }

    pub(super) fn bg_elevated(&self) -> Rgba {
        self.ui_tokens().bg_card
    }

    pub(super) fn divider_color(&self) -> Rgba {
        self.ui_tokens().row_separator
    }

    pub(super) fn card_border_color(&self) -> Rgba {
        self.ui_tokens().card_border
    }

    pub(super) fn sidebar_selection_bg(&self) -> Rgba {
        self.ui_tokens().accent
    }

    pub(super) fn bg_input(&self) -> Rgba {
        self.ui_tokens().bg_input
    }

    pub(super) fn bg_hover(&self) -> Rgba {
        self.ui_tokens().bg_hover
    }

    pub(super) fn text_primary(&self) -> Rgba {
        self.ui_tokens().text_primary
    }

    pub(super) fn text_secondary(&self) -> Rgba {
        self.ui_tokens().text_secondary
    }

    pub(super) fn text_muted(&self) -> Rgba {
        self.ui_tokens().text_muted
    }

    pub(super) fn border_color(&self) -> Rgba {
        self.ui_tokens().border
    }

    pub(super) fn accent(&self) -> Rgba {
        self.ui_tokens().accent
    }

    pub(super) fn accent_with_alpha(&self, alpha: f32) -> Rgba {
        termy_ui::theme::with_alpha(self.accent(), alpha)
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
        let mut track = self.text_primary();
        track.a = SETTINGS_SCROLLBAR_TRACK_ALPHA;

        let mut thumb = self.text_primary();
        thumb.a = SETTINGS_SCROLLBAR_THUMB_ALPHA;

        let mut active_thumb = self.text_primary();
        active_thumb.a = SETTINGS_SCROLLBAR_THUMB_ACTIVE_ALPHA;

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
