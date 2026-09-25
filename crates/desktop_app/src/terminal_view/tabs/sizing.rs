use super::*;
use crate::terminal_view::tab_strip::state::TabStripOrientation;

impl TerminalView {
    pub(in crate::terminal_view) fn evict_inactive_tab_render_caches(
        tabs: &[TerminalTab],
        active_tab: usize,
    ) {
        for (tab_index, tab) in tabs.iter().enumerate() {
            if tab_index != active_tab {
                tab.clear_render_caches();
            }
        }
    }

    pub(in crate::terminal_view) fn evict_inactive_terminal_render_caches(&self) {
        Self::evict_inactive_tab_render_caches(&self.session.tabs, self.session.active_tab);

        // The active workspace keeps its tabs in `self.session.tabs`; every tab stored
        // on a workspace entry is hidden and can rebuild lazily when restored.
        for workspace in &self.session.workspaces {
            for tab in &workspace.tabs {
                tab.clear_render_caches();
            }
        }

        // Native zoom temporarily stashes the other panes outside the active
        // tab. They are hidden for the lifetime of the snapshot as well.
        for snapshot in self.session.native_pane_zoom_snapshots.values() {
            for pane in &snapshot.other_panes {
                pane.render_cache.borrow_mut().clear();
            }
        }
    }

    pub(in crate::terminal_view) fn sync_native_terminal_wakeup_interest(&self) {
        if self.runtime_kind() != RuntimeKind::Native {
            return;
        }

        for (tab_index, tab) in self.session.tabs.iter().enumerate() {
            let tab_visible = tab_index == self.session.active_tab;
            for pane in &tab.panes {
                pane.terminal().set_wakeup_enabled(tab_visible);
            }
        }

        // Workspace switching moves the old strip into `WorkspaceEntry::tabs`.
        // Those panes must stop driving render wakeups just like ordinary
        // inactive tabs; their event queues are still drained by the shared
        // workspace drain pass, and damage is retained until reactivation.
        for workspace in &self.session.workspaces {
            for tab in &workspace.tabs {
                for pane in &tab.panes {
                    pane.terminal().set_wakeup_enabled(false);
                }
            }
        }

        for snapshot in self.session.native_pane_zoom_snapshots.values() {
            for pane in &snapshot.other_panes {
                pane.terminal().set_wakeup_enabled(false);
            }
        }
    }

    pub(crate) fn sync_tab_strip_for_active_tab(&mut self) {
        if self.tab_strip_orientation() == TabStripOrientation::Horizontal {
            self.sync_tab_display_widths_for_viewport_if_needed(
                self.tab_strip.horizontal_layout_last_synced_viewport_width,
            );
        }
        self.scroll_active_tab_into_view(self.tab_strip_orientation());
        self.evict_inactive_terminal_render_caches();
        self.sync_native_terminal_wakeup_interest();
    }

    pub(crate) fn tab_strip_fixed_content_width(&self) -> f32 {
        let tabs_width: f32 = self.session.tabs.iter().map(|tab| tab.display_width).sum();
        let gaps = TAB_ITEM_GAP * self.session.tabs.len().saturating_sub(1) as f32;
        TAB_HORIZONTAL_PADDING + tabs_width + gaps
    }

    pub(crate) fn tab_strip_expected_max_scroll_for_viewport(&self, viewport_width: f32) -> f32 {
        (self.tab_strip_fixed_content_width() - viewport_width.max(0.0)).max(0.0)
    }

    pub(crate) fn tab_strip_scroll_max_x(&self) -> f32 {
        self.tab_strip_expected_max_scroll_for_viewport(
            self.tab_strip.horizontal_layout_last_synced_viewport_width,
        )
    }

    fn target_scroll_for_active_tab_bounds(
        current_scroll: f32,
        viewport_width: f32,
        tab_left: f32,
        tab_right: f32,
    ) -> f32 {
        let mut target_scroll = current_scroll;
        // A tab can overflow both edges after active-width recalculation.
        // Bias right-edge overflow first so switching/creating tabs near the
        // end does not snap backward toward the tab-strip start.
        if tab_right > current_scroll + viewport_width {
            target_scroll = tab_right - viewport_width;
        } else if tab_left < current_scroll {
            target_scroll = tab_left;
        }
        target_scroll
    }

    pub(crate) fn scroll_active_tab_into_view(&self, orientation: TabStripOrientation) {
        if self.session.active_tab >= self.session.tabs.len() {
            return;
        }

        match orientation {
            TabStripOrientation::Horizontal => {
                let viewport_width = self
                    .tab_strip
                    .horizontal_layout_last_synced_viewport_width
                    .max(0.0);
                if viewport_width <= f32::EPSILON {
                    return;
                }

                let max_scroll = self.tab_strip_scroll_max_x();
                let mut tab_left = TAB_HORIZONTAL_PADDING;
                for (index, tab) in self.session.tabs.iter().enumerate() {
                    let tab_right = tab_left + tab.display_width;
                    if index == self.session.active_tab {
                        let offset = self.tab_strip.horizontal_scroll_handle.offset();
                        let current_scroll = -Into::<f32>::into(offset.x);
                        let target_scroll = Self::target_scroll_for_active_tab_bounds(
                            current_scroll,
                            viewport_width,
                            tab_left,
                            tab_right,
                        );

                        let clamped_scroll = target_scroll.clamp(0.0, max_scroll);
                        let next_offset_x = -clamped_scroll;
                        let current_offset_x: f32 = offset.x.into();
                        if (next_offset_x - current_offset_x).abs() > f32::EPSILON {
                            self.tab_strip
                                .horizontal_scroll_handle
                                .set_offset(point(px(next_offset_x), offset.y));
                        }
                        return;
                    }
                    tab_left = tab_right + TAB_ITEM_GAP;
                }
            }
            TabStripOrientation::Vertical => {
                let viewport_height = self
                    .tab_strip
                    .vertical_layout_last_synced_viewport_height
                    .max(0.0);
                if viewport_height <= f32::EPSILON {
                    return;
                }

                let max_scroll = self.tab_strip_scroll_max_y();
                let mut tab_top = SIDEBAR_TAB_PADDING_Y;
                for index in 0..self.session.tabs.len() {
                    let tab_bottom = tab_top + SIDEBAR_TAB_ROW_HEIGHT;
                    if index == self.session.active_tab {
                        let offset = self.tab_strip.vertical_scroll_handle.offset();
                        let current_scroll = -Into::<f32>::into(offset.y);
                        // target_scroll_for_active_tab_bounds is axis-agnostic.
                        let target_scroll = Self::target_scroll_for_active_tab_bounds(
                            current_scroll,
                            viewport_height,
                            tab_top,
                            tab_bottom,
                        );
                        let clamped_scroll = target_scroll.clamp(0.0, max_scroll);
                        let next_offset_y = -clamped_scroll;
                        let current_offset_y: f32 = offset.y.into();
                        if (next_offset_y - current_offset_y).abs() > f32::EPSILON {
                            self.tab_strip
                                .vertical_scroll_handle
                                .set_offset(point(offset.x, px(next_offset_y)));
                        }
                        return;
                    }
                    tab_top = tab_bottom + SIDEBAR_TAB_ROW_GAP;
                }
            }
        }
    }

    pub(crate) fn tab_strip_vertical_content_height(&self) -> f32 {
        let rows = self.session.tabs.len();
        if rows == 0 {
            return 0.0;
        }
        (rows as f32 * SIDEBAR_TAB_ROW_HEIGHT)
            + (SIDEBAR_TAB_ROW_GAP * (rows - 1) as f32)
            + (SIDEBAR_TAB_PADDING_Y * 2.0)
    }

    pub(crate) fn tab_strip_scroll_max_y(&self) -> f32 {
        let viewport_height = self
            .tab_strip
            .vertical_layout_last_synced_viewport_height
            .max(0.0);
        (self.tab_strip_vertical_content_height() - viewport_height).max(0.0)
    }

    pub(crate) fn scroll_tab_strip_vertically_by(&mut self, delta_y: f32) -> bool {
        let max_scroll = self.tab_strip_scroll_max_y();
        let offset = self.tab_strip.vertical_scroll_handle.offset();
        let current_offset_y: f32 = offset.y.into();
        // tab_strip_offset_x_for_delta is axis-agnostic (1D clamp).
        let Some(next_offset_y) =
            Self::tab_strip_offset_x_for_delta(current_offset_y, delta_y, max_scroll)
        else {
            return false;
        };

        self.tab_strip
            .vertical_scroll_handle
            .set_offset(point(offset.x, px(next_offset_y)));
        true
    }

    fn tab_strip_overflow_state_for_scroll(
        scroll_x: f32,
        max_scroll_x: f32,
    ) -> TabStripOverflowState {
        let max_scroll = max_scroll_x.max(0.0);
        if max_scroll <= TAB_STRIP_SCROLL_EPSILON {
            return TabStripOverflowState::default();
        }

        let clamped_scroll = scroll_x.clamp(0.0, max_scroll);
        TabStripOverflowState {
            left: clamped_scroll > TAB_STRIP_SCROLL_EPSILON,
            right: (max_scroll - clamped_scroll) > TAB_STRIP_SCROLL_EPSILON,
        }
    }

    pub(crate) fn tab_strip_overflow_state(&self) -> TabStripOverflowState {
        let offset = self.tab_strip.horizontal_scroll_handle.offset();
        let scroll_x = -Into::<f32>::into(offset.x);
        let max_scroll = self.tab_strip_scroll_max_x();
        Self::tab_strip_overflow_state_for_scroll(scroll_x, max_scroll)
    }

    fn tab_strip_offset_x_for_delta(
        current_offset_x: f32,
        delta_x: f32,
        max_scroll: f32,
    ) -> Option<f32> {
        if delta_x.abs() <= f32::EPSILON {
            return None;
        }

        let bounded_max = max_scroll.max(0.0);
        if bounded_max <= TAB_STRIP_SCROLL_EPSILON {
            return None;
        }

        let clamped_current = current_offset_x.clamp(-bounded_max, 0.0);
        let next_offset = (clamped_current + delta_x).clamp(-bounded_max, 0.0);
        ((next_offset - clamped_current).abs() > f32::EPSILON).then_some(next_offset)
    }

    pub(crate) fn scroll_tab_strip_by(&mut self, delta_x: f32) -> bool {
        let max_scroll = self.tab_strip_scroll_max_x();
        let offset = self.tab_strip.horizontal_scroll_handle.offset();
        let current_offset_x: f32 = offset.x.into();
        let Some(next_offset_x) =
            Self::tab_strip_offset_x_for_delta(current_offset_x, delta_x, max_scroll)
        else {
            return false;
        };

        self.tab_strip
            .horizontal_scroll_handle
            .set_offset(point(px(next_offset_x), offset.y));
        true
    }

    pub(crate) fn effective_tab_max_width_for_viewport(
        viewport_width: f32,
        tab_count: usize,
    ) -> f32 {
        let tab_count = tab_count.max(1);
        let gap_width = TAB_ITEM_GAP * tab_count.saturating_sub(1) as f32;
        let content_width =
            (viewport_width - (TAB_HORIZONTAL_PADDING * 2.0) - gap_width).max(TAB_MIN_WIDTH);
        let share = content_width / tab_count as f32;

        // Tabs never grow past TAB_MAX_WIDTH: with plenty of room each tab caps
        // at the max instead of stretching to fill the strip; when the strip is
        // crowded the equal share shrinks them down toward TAB_MIN_WIDTH.
        share.clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
    }

    fn tab_display_width_for_text_px_with_close_policy(
        text_width_px: f32,
        max_width: f32,
        reserve_close_slot: bool,
    ) -> f32 {
        let text_width = if text_width_px.is_finite() {
            text_width_px.max(0.0)
        } else {
            0.0
        };
        let close_slot_width = if reserve_close_slot {
            TAB_CLOSE_SLOT_WIDTH
        } else {
            0.0
        };
        let base_width =
            (TAB_TEXT_PADDING_X * 2.0) + TAB_LEADING_SLOT_WIDTH + text_width + close_slot_width;
        let slack_start = TAB_MIN_WIDTH - TAB_TITLE_LAYOUT_SLACK_PX;
        let slack_end = TAB_MIN_WIDTH + TAB_TITLE_LAYOUT_SLACK_PX;
        let slack_span = (slack_end - slack_start).max(f32::EPSILON);
        let slack_factor = ((slack_end - base_width) / slack_span).clamp(0.0, 1.0);
        let effective_slack = TAB_TITLE_LAYOUT_SLACK_PX * slack_factor;
        let width = base_width + effective_slack;
        width.clamp(TAB_MIN_WIDTH, max_width.max(TAB_MIN_WIDTH))
    }

    pub(crate) fn tab_display_width_for_text_px_with_max(
        text_width_px: f32,
        max_width: f32,
    ) -> f32 {
        Self::tab_display_width_for_text_px_with_close_policy(text_width_px, max_width, true)
    }

    pub(crate) fn tab_display_width_for_text_px_without_close_with_max(
        text_width_px: f32,
        max_width: f32,
    ) -> f32 {
        Self::tab_display_width_for_text_px_with_close_policy(text_width_px, max_width, false)
    }

    pub(crate) fn tab_title_text_area_width(tab_width: f32, close_slot_width: f32) -> f32 {
        (tab_width - (TAB_TEXT_PADDING_X * 2.0) - close_slot_width).max(0.0)
    }

    pub(crate) fn sync_tab_title_text_widths(&mut self, measured_text_widths: &[f32]) -> bool {
        debug_assert_eq!(measured_text_widths.len(), self.session.tabs.len());
        let mut changed = false;

        for (index, tab) in self.session.tabs.iter_mut().enumerate() {
            let Some(width) = measured_text_widths.get(index).copied() else {
                continue;
            };
            let width = if width.is_finite() {
                width.max(0.0)
            } else {
                0.0
            };
            if (tab.title_text_width - width).abs() <= 0.01 {
                continue;
            }
            tab.title_text_width = width;
            changed = true;
        }

        if changed {
            self.mark_tab_strip_layout_dirty();
        }

        changed
    }

    pub(crate) fn tab_reserves_close_slot_for_layout(
        tab_width_mode: TabWidthMode,
        tab_close_visibility: TabCloseVisibility,
        _is_active: bool,
        is_pinned: bool,
    ) -> bool {
        if is_pinned {
            return true;
        }

        match tab_width_mode {
            TabWidthMode::Stable | TabWidthMode::Uniform => true,
            TabWidthMode::ActiveGrow | TabWidthMode::ActiveGrowSticky => match tab_close_visibility
            {
                TabCloseVisibility::Always
                | TabCloseVisibility::ActiveHover
                | TabCloseVisibility::Hover => true,
            },
        }
    }

    fn resolve_tab_width_for_mode(
        tab_width_mode: TabWidthMode,
        text_width_px: f32,
        effective_max: f32,
        reserve_close_slot: bool,
        sticky_title_width: f32,
    ) -> (f32, f32) {
        let capped_max = effective_max.max(TAB_MIN_WIDTH);
        let title_only_width = Self::tab_display_width_for_text_px_without_close_with_max(
            text_width_px,
            effective_max,
        );
        let close_policy_width = Self::tab_display_width_for_text_px_with_close_policy(
            text_width_px,
            effective_max,
            reserve_close_slot,
        );

        match tab_width_mode {
            TabWidthMode::Stable | TabWidthMode::ActiveGrow => {
                (close_policy_width, title_only_width)
            }
            TabWidthMode::ActiveGrowSticky => {
                let next_sticky_width = sticky_title_width.max(title_only_width).min(capped_max);
                let next_width = if reserve_close_slot {
                    close_policy_width.max(next_sticky_width).min(capped_max)
                } else {
                    next_sticky_width
                };
                (next_width, next_sticky_width)
            }
            TabWidthMode::Uniform => (capped_max, capped_max),
        }
    }

    pub(crate) fn sync_tab_display_widths_for_viewport(&mut self, viewport_width: f32) -> bool {
        let viewport_width = if viewport_width.is_finite() {
            viewport_width.max(0.0)
        } else {
            0.0
        };
        let effective_max =
            Self::effective_tab_max_width_for_viewport(viewport_width, self.session.tabs.len());
        let mut changed = false;
        let tab_width_mode = self.tab_width_mode;
        let tab_close_visibility = self.tab_close_visibility;
        let renaming_tab = self.renaming_tab;
        // A tab being renamed expands past the normal cap so the inline editor
        // has room. It grows with the typed text up to TAB_RENAME_MAX_WIDTH,
        // floored at TAB_RENAME_MIN_WIDTH, and is always bounded by the viewport
        // so it never overflows the strip.
        let viewport_cap = viewport_width.max(TAB_MIN_WIDTH);
        let rename_cap = TAB_RENAME_MAX_WIDTH.min(viewport_cap);
        let rename_floor = TAB_RENAME_MIN_WIDTH.min(viewport_cap);

        for (index, tab) in self.session.tabs.iter_mut().enumerate() {
            let is_active = index == self.session.active_tab;
            let reserve_close_slot = Self::tab_reserves_close_slot_for_layout(
                tab_width_mode,
                tab_close_visibility,
                is_active,
                tab.pinned,
            );
            let (next_width, next_sticky_width) = Self::resolve_tab_width_for_mode(
                tab_width_mode,
                tab.title_text_width,
                effective_max,
                reserve_close_slot,
                tab.sticky_title_width,
            );
            let next_width = next_width.max(TAB_MIN_WIDTH);
            let next_width = if renaming_tab == Some(index) {
                // Size to the editor text (no close slot is shown while
                // renaming), then floor/cap so short names still get room and
                // long names stay within the strip.
                let rename_fit = Self::tab_display_width_for_text_px_without_close_with_max(
                    tab.title_text_width,
                    rename_cap,
                );
                next_width.max(rename_fit).max(rename_floor).min(rename_cap)
            } else {
                next_width
            };
            let next_sticky_width = next_sticky_width.max(TAB_MIN_WIDTH);
            tab.sticky_title_width = next_sticky_width;
            if (tab.display_width - next_width).abs() <= f32::EPSILON {
                continue;
            }

            tab.display_width = next_width;
            changed = true;
        }

        changed
    }

    pub(crate) fn mark_tab_strip_layout_dirty(&mut self) {
        self.tab_strip.invalidate_layouts();
    }

    pub(crate) fn sync_tab_display_widths_for_viewport_if_needed(
        &mut self,
        viewport_width: f32,
    ) -> bool {
        let clamped_viewport = if viewport_width.is_finite() {
            viewport_width.max(0.0)
        } else {
            0.0
        };
        let viewport_unchanged =
            (self.tab_strip.horizontal_layout_last_synced_viewport_width - clamped_viewport).abs()
                <= f32::EPSILON;
        let revision_unchanged = self.tab_strip.horizontal_layout_last_synced_revision
            == self.tab_strip.horizontal_layout_revision;
        if viewport_unchanged && revision_unchanged {
            return false;
        }

        let changed = self.sync_tab_display_widths_for_viewport(clamped_viewport);
        self.tab_strip.horizontal_layout_last_synced_viewport_width = clamped_viewport;
        self.tab_strip.horizontal_layout_last_synced_revision =
            self.tab_strip.horizontal_layout_revision;
        changed
    }

    pub(crate) fn tab_shows_close(
        close_visibility: TabCloseVisibility,
        is_active: bool,
        hovered_tab: Option<usize>,
        hovered_tab_close: Option<usize>,
        index: usize,
    ) -> bool {
        match close_visibility {
            TabCloseVisibility::ActiveHover => {
                is_active || hovered_tab == Some(index) || hovered_tab_close == Some(index)
            }
            TabCloseVisibility::Hover => {
                hovered_tab == Some(index) || hovered_tab_close == Some(index)
            }
            TabCloseVisibility::Always => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn synthetic_title_width_px(title: &str) -> f32 {
        title.chars().count() as f32 * 7.0
    }

    fn assert_float_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.0001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn tab_display_width_for_text_px_clamps_to_min() {
        let width = TerminalView::tab_display_width_for_text_px_with_max(1.0, TAB_MAX_WIDTH);
        assert_eq!(width, TAB_MIN_WIDTH);
    }

    #[test]
    fn tab_display_width_for_text_px_clamps_to_max() {
        let width = TerminalView::tab_display_width_for_text_px_with_max(4000.0, TAB_MAX_WIDTH);
        assert_eq!(width, TAB_MAX_WIDTH);
    }

    #[test]
    fn tab_display_width_for_text_px_tapers_slack_for_short_titles() {
        let long_text_width = 120.0;
        let long_width = TerminalView::tab_display_width_for_text_px_with_max(
            long_text_width,
            TAB_MAX_WIDTH * 2.0,
        );
        let expected_long = (TAB_TEXT_PADDING_X * 2.0)
            + TAB_LEADING_SLOT_WIDTH
            + long_text_width
            + TAB_CLOSE_SLOT_WIDTH;
        assert_eq!(long_width, expected_long);

        let short_text_width = 34.0;
        let short_width = TerminalView::tab_display_width_for_text_px_with_max(
            short_text_width,
            TAB_MAX_WIDTH * 2.0,
        );
        let short_base = (TAB_TEXT_PADDING_X * 2.0)
            + TAB_LEADING_SLOT_WIDTH
            + short_text_width
            + TAB_CLOSE_SLOT_WIDTH;
        assert!(short_width > short_base);
        assert!(short_width < short_base + TAB_TITLE_LAYOUT_SLACK_PX);
    }

    #[test]
    fn tab_display_width_for_text_px_is_monotonic_near_slack_transition() {
        let width_7 = TerminalView::tab_display_width_for_text_px_with_max(64.0, 512.0);
        let width_8 = TerminalView::tab_display_width_for_text_px_with_max(74.0, 512.0);
        let width_9 = TerminalView::tab_display_width_for_text_px_with_max(84.0, 512.0);

        assert!(width_7 < width_8);
        assert!(width_8 < width_9);
    }

    #[test]
    fn tab_display_width_for_text_px_with_max_uses_provided_cap() {
        let width = TerminalView::tab_display_width_for_text_px_with_max(4000.0, 512.0);
        assert_eq!(width, 512.0);
    }

    #[test]
    fn effective_tab_max_width_caps_at_max_for_sparse_tabs() {
        let effective = TerminalView::effective_tab_max_width_for_viewport(1600.0, 1);
        assert_float_eq(effective, TAB_MAX_WIDTH);
    }

    #[test]
    fn effective_tab_max_width_shrinks_for_crowded_tabs() {
        let effective = TerminalView::effective_tab_max_width_for_viewport(1600.0, 10);
        let expected = (1600.0 - (TAB_HORIZONTAL_PADDING * 2.0) - (TAB_ITEM_GAP * 9.0)) / 10.0;
        assert_float_eq(effective, expected);
        assert!(effective < TAB_MAX_WIDTH);
    }

    #[test]
    fn effective_tab_max_width_never_drops_below_min_for_crowded_tabs() {
        let effective = TerminalView::effective_tab_max_width_for_viewport(120.0, 12);
        assert_eq!(effective, TAB_MIN_WIDTH);
    }

    #[test]
    fn effective_tab_max_width_caps_at_max_for_very_wide_viewport() {
        let effective = TerminalView::effective_tab_max_width_for_viewport(4000.0, 1);
        assert_float_eq(effective, TAB_MAX_WIDTH);
    }

    #[test]
    fn tab_shows_close_for_active_or_hovered() {
        assert!(TerminalView::tab_shows_close(
            TabCloseVisibility::ActiveHover,
            true,
            None,
            None,
            1,
        ));
        assert!(TerminalView::tab_shows_close(
            TabCloseVisibility::ActiveHover,
            false,
            Some(1),
            None,
            1,
        ));
        assert!(TerminalView::tab_shows_close(
            TabCloseVisibility::ActiveHover,
            false,
            None,
            Some(1),
            1,
        ));
        assert!(!TerminalView::tab_shows_close(
            TabCloseVisibility::ActiveHover,
            false,
            Some(2),
            None,
            1,
        ));
        assert!(!TerminalView::tab_shows_close(
            TabCloseVisibility::ActiveHover,
            false,
            None,
            Some(2),
            1,
        ));
    }

    #[test]
    fn tab_shows_close_hover_mode_ignores_active_state() {
        assert!(!TerminalView::tab_shows_close(
            TabCloseVisibility::Hover,
            true,
            None,
            None,
            0,
        ));
        assert!(TerminalView::tab_shows_close(
            TabCloseVisibility::Hover,
            false,
            Some(0),
            None,
            0,
        ));
    }

    #[test]
    fn tab_shows_close_always_mode_always_true() {
        assert!(TerminalView::tab_shows_close(
            TabCloseVisibility::Always,
            false,
            None,
            None,
            2,
        ));
    }

    #[test]
    fn active_grow_mode_reserves_close_slot_for_active_only() {
        let text_width = synthetic_title_width_px("~/Desktop");
        let max_width = 512.0;
        let expected_active = TerminalView::tab_display_width_for_text_px_with_close_policy(
            text_width, max_width, true,
        );
        let expected_inactive = TerminalView::tab_display_width_for_text_px_with_close_policy(
            text_width, max_width, false,
        );

        let (active_width, active_sticky) = TerminalView::resolve_tab_width_for_mode(
            TabWidthMode::ActiveGrow,
            text_width,
            max_width,
            true,
            TAB_MIN_WIDTH,
        );
        let (inactive_width, inactive_sticky) = TerminalView::resolve_tab_width_for_mode(
            TabWidthMode::ActiveGrow,
            text_width,
            max_width,
            false,
            TAB_MIN_WIDTH,
        );

        assert_eq!(active_width, expected_active);
        assert_eq!(inactive_width, expected_inactive);
        assert_eq!(active_sticky, expected_inactive);
        assert_eq!(inactive_sticky, expected_inactive);
        assert!(active_width > inactive_width);
    }

    #[test]
    fn active_grow_sticky_drops_close_only_extra_when_inactive() {
        let text_width = synthetic_title_width_px("~/Desktop");
        let max_width = 512.0;
        let title_only = TerminalView::tab_display_width_for_text_px_without_close_with_max(
            text_width, max_width,
        );
        let with_close = TerminalView::tab_display_width_for_text_px_with_close_policy(
            text_width, max_width, true,
        );

        let (active_width, sticky_after_active) = TerminalView::resolve_tab_width_for_mode(
            TabWidthMode::ActiveGrowSticky,
            text_width,
            max_width,
            true,
            title_only,
        );
        let (inactive_width, sticky_after_inactive) = TerminalView::resolve_tab_width_for_mode(
            TabWidthMode::ActiveGrowSticky,
            text_width,
            max_width,
            false,
            sticky_after_active,
        );

        assert_eq!(active_width, with_close);
        assert_eq!(inactive_width, title_only);
        assert_eq!(sticky_after_inactive, title_only);
    }

    #[test]
    fn active_grow_sticky_respects_manual_sticky_reset_to_current_title() {
        let long_text_width = synthetic_title_width_px(
            "~/Desktop/claudeCode/claude-code-provider-proxy/docs/test2/test4/test4",
        );
        let short_text_width = synthetic_title_width_px("~/Desktop");
        let max_width = 512.0;
        let short_title_only = TerminalView::tab_display_width_for_text_px_without_close_with_max(
            short_text_width,
            max_width,
        );

        let (_, sticky_after_long) = TerminalView::resolve_tab_width_for_mode(
            TabWidthMode::ActiveGrowSticky,
            long_text_width,
            max_width,
            false,
            0.0,
        );
        let (width_without_reset, _) = TerminalView::resolve_tab_width_for_mode(
            TabWidthMode::ActiveGrowSticky,
            short_text_width,
            max_width,
            false,
            sticky_after_long,
        );
        let (width_with_reset, sticky_with_reset) = TerminalView::resolve_tab_width_for_mode(
            TabWidthMode::ActiveGrowSticky,
            short_text_width,
            max_width,
            false,
            0.0,
        );

        assert!(width_without_reset > width_with_reset);
        assert_eq!(width_with_reset, short_title_only);
        assert_eq!(sticky_with_reset, short_title_only);
    }

    #[test]
    fn active_grow_sticky_caps_sticky_width_under_pressure() {
        let text_width = synthetic_title_width_px("tab");
        let effective_max = TAB_MIN_WIDTH + 10.0;
        let prior_sticky = 260.0;

        let (next_width, next_sticky) = TerminalView::resolve_tab_width_for_mode(
            TabWidthMode::ActiveGrowSticky,
            text_width,
            effective_max,
            false,
            prior_sticky,
        );

        assert_eq!(next_sticky, effective_max);
        assert_eq!(next_width, effective_max);
    }

    #[test]
    fn tab_width_modes_never_resolve_below_min_when_cap_is_tight() {
        for tab_width_mode in [
            TabWidthMode::Stable,
            TabWidthMode::ActiveGrow,
            TabWidthMode::ActiveGrowSticky,
            TabWidthMode::Uniform,
        ] {
            let (next_width, next_sticky) =
                TerminalView::resolve_tab_width_for_mode(tab_width_mode, 8.0, 12.0, true, 0.0);

            assert!(
                next_width >= TAB_MIN_WIDTH,
                "{tab_width_mode:?} width resolved below min: {next_width}"
            );
            assert!(
                next_sticky >= TAB_MIN_WIDTH,
                "{tab_width_mode:?} sticky width resolved below min: {next_sticky}"
            );
        }
    }

    #[test]
    fn tab_strip_overflow_state_reports_none_without_scroll_range() {
        assert_eq!(
            TerminalView::tab_strip_overflow_state_for_scroll(0.0, 0.0),
            TabStripOverflowState::default()
        );
    }

    #[test]
    fn tab_strip_overflow_state_reports_right_overflow_at_start() {
        assert_eq!(
            TerminalView::tab_strip_overflow_state_for_scroll(0.0, 120.0),
            TabStripOverflowState {
                left: false,
                right: true,
            }
        );
    }

    #[test]
    fn tab_strip_overflow_state_reports_left_overflow_at_end() {
        assert_eq!(
            TerminalView::tab_strip_overflow_state_for_scroll(120.0, 120.0),
            TabStripOverflowState {
                left: true,
                right: false,
            }
        );
    }

    #[test]
    fn tab_strip_overflow_state_reports_both_when_scrolled_in_middle() {
        assert_eq!(
            TerminalView::tab_strip_overflow_state_for_scroll(42.0, 120.0),
            TabStripOverflowState {
                left: true,
                right: true,
            }
        );
    }

    #[test]
    fn active_tab_target_scroll_prefers_right_overflow_when_both_edges_overflow() {
        let current_scroll = 120.0;
        let viewport_width = 80.0;
        let tab_left = 100.0;
        let tab_right = 220.0;

        // Left overflow: tab_left(100) < current_scroll(120)
        // Right overflow: tab_right(220) > current_scroll + viewport(200)
        assert_float_eq(
            TerminalView::target_scroll_for_active_tab_bounds(
                current_scroll,
                viewport_width,
                tab_left,
                tab_right,
            ),
            140.0,
        );
    }

    #[test]
    fn active_tab_target_scroll_uses_left_overflow_when_only_left_overflows() {
        assert_float_eq(
            TerminalView::target_scroll_for_active_tab_bounds(120.0, 80.0, 100.0, 180.0),
            100.0,
        );
    }

    #[test]
    fn active_tab_target_scroll_uses_right_overflow_when_only_right_overflows() {
        assert_float_eq(
            TerminalView::target_scroll_for_active_tab_bounds(120.0, 80.0, 130.0, 230.0),
            150.0,
        );
    }

    #[test]
    fn tab_strip_offset_x_for_delta_clamps_to_left_limit() {
        assert_eq!(
            TerminalView::tab_strip_offset_x_for_delta(-24.0, 96.0, 120.0),
            Some(0.0)
        );
    }

    #[test]
    fn tab_strip_offset_x_for_delta_clamps_to_right_limit() {
        assert_eq!(
            TerminalView::tab_strip_offset_x_for_delta(-96.0, -64.0, 120.0),
            Some(-120.0)
        );
    }

    #[test]
    fn tab_strip_offset_x_for_delta_is_noop_without_scroll_range() {
        assert_eq!(
            TerminalView::tab_strip_offset_x_for_delta(0.0, 24.0, 0.0),
            None
        );
    }

    #[test]
    fn tab_strip_geometry_positions_action_rail_and_button_bounds() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        assert!(geometry.row_start_x > 0.0);
        assert!(geometry.tabs_viewport_width > 0.0);
        assert_float_eq(geometry.tabs_viewport_end_x(), geometry.gutter_start_x);
        assert_float_eq(geometry.gutter_end_x(), geometry.action_rail_start_x);
        assert_float_eq(
            geometry.action_rail_end_x(),
            geometry.action_rail_start_x + geometry.action_rail_width,
        );
        assert!(geometry.button_start_x >= geometry.action_rail_start_x);
        assert!(geometry.button_end_x <= geometry.action_rail_end_x());
        assert!(geometry.button_start_y >= TOP_STRIP_CONTENT_OFFSET_Y);
    }

    #[test]
    fn tab_strip_geometry_clamps_action_rail_for_narrow_viewport() {
        let viewport_width = TerminalView::titlebar_left_padding_for_platform()
            + TerminalView::titlebar_right_padding_for_platform()
            + 24.0;
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(viewport_width);

        assert_float_eq(geometry.row_width, 24.0);
        assert_float_eq(geometry.action_rail_width, 24.0);
        assert_float_eq(geometry.tabs_viewport_width, 0.0);
        assert!(geometry.button_start_x >= geometry.action_rail_start_x);
        assert!(geometry.button_end_x <= geometry.action_rail_end_x());
    }

    #[test]
    fn tab_strip_geometry_uses_half_open_bounds_between_regions() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        let boundary_x = geometry.tabs_viewport_end_x();
        assert!(!geometry.contains_tabs_viewport_x(boundary_x));
        if geometry.gutter_width > 0.0 {
            assert!(geometry.contains_gutter_x(boundary_x));
        }
        assert!(geometry.contains_tabs_viewport_x(boundary_x - 1.0));
        if geometry.action_rail_width > 0.0 {
            assert!(geometry.contains_action_rail_x(geometry.gutter_end_x()));
        }
    }

    #[test]
    fn tab_strip_pointer_transform_accounts_for_non_zero_origin() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        assert_float_eq(
            TerminalView::tab_strip_pointer_x_from_window_x_for_geometry(
                geometry.row_start_x + 24.0,
                geometry,
            ),
            24.0,
        );
        assert_float_eq(
            TerminalView::tab_strip_pointer_x_from_window_x_for_geometry(0.0, geometry),
            0.0,
        );
    }

    #[test]
    fn tab_strip_geometry_detects_new_tab_button_hit_bounds() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(960.0);
        let button_center_x = (geometry.button_start_x + geometry.button_end_x) * 0.5;
        let button_center_y = (geometry.button_start_y + geometry.button_end_y) * 0.5;
        assert!(geometry.contains_action_rail_x(button_center_x));
        assert!(geometry.new_tab_button_contains(button_center_x, button_center_y));
        assert!(!geometry.new_tab_button_contains(geometry.button_start_x - 1.0, button_center_y));
    }
}
