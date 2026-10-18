use super::*;
use crate::terminal_view::tab_strip::state::TabStripOrientation;

impl TerminalView {
    fn clear_tab_drag_preview_state(&mut self) {
        self.tab_strip.drag_preview.clear();
    }

    fn ensure_tab_drag_autoscroll_animation(&mut self, cx: &mut Context<Self>) {
        if self.tab_strip.drag_preview.autoscroll_animating() {
            return;
        }
        self.tab_strip.drag_preview.start_autoscroll_animation();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let Ok(keep_animating) = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        if !view.tab_strip.drag_preview.autoscroll_animating()
                            || view.tab_strip.drag.is_none()
                        {
                            view.tab_strip.drag_preview.stop_autoscroll_animation();
                            return false;
                        }

                        let Some(pointer_primary_axis) =
                            view.tab_strip.drag_preview.pointer_primary_axis()
                        else {
                            view.tab_strip.drag_preview.stop_autoscroll_animation();
                            return false;
                        };
                        let viewport_extent = view.tab_strip.drag_preview.viewport_extent();
                        let Some(orientation) = view.tab_strip.drag.map(|drag| drag.orientation)
                        else {
                            view.tab_strip.drag_preview.stop_autoscroll_animation();
                            return false;
                        };

                        let scrolled = view.auto_scroll_tab_strip_during_drag(
                            orientation,
                            pointer_primary_axis,
                            viewport_extent,
                        );
                        let marker_changed =
                            view.update_tab_drag_marker(orientation, pointer_primary_axis, cx);
                        if scrolled && !marker_changed {
                            cx.notify();
                        }
                        if !scrolled {
                            view.tab_strip.drag_preview.stop_autoscroll_animation();
                            return false;
                        }
                        true
                    })
                }) else {
                    break;
                };

                if !keep_animating {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn begin_tab_drag(&mut self, index: usize, orientation: TabStripOrientation) {
        if index < self.session.tabs.len() {
            self.disarm_titlebar_window_move();
            self.clear_tab_drag_preview_state();
            self.tab_strip.drag = Some(TabDragState {
                source_index: index,
                drop_slot: None,
                orientation,
            });
        }
    }

    pub(crate) fn finish_tab_drag(&mut self) -> bool {
        let marker_was_visible = self
            .tab_strip
            .drag
            .as_ref()
            .and_then(|drag| drag.drop_slot)
            .is_some();
        self.tab_strip.drag = None;
        self.clear_tab_drag_preview_state();
        marker_was_visible
    }

    // Axis-agnostic 1D drop-slot mapping: given each tab's extent along the
    // primary axis (widths for horizontal, row heights for vertical), the
    // pointer position on that axis, the scroll offset, and the strip's leading
    // inset / inter-tab gap on that axis, return the insertion slot.
    fn tab_drop_slot_from_pointer_primary_axis_for_extents(
        tab_extents: impl IntoIterator<Item = f32>,
        pointer_primary_axis: f32,
        scroll_offset: f32,
        leading_inset: f32,
        tab_gap: f32,
    ) -> usize {
        let mut leading_edge = leading_inset + scroll_offset;
        let mut slot = 0;

        for extent in tab_extents {
            let midpoint = leading_edge + (extent * 0.5);
            if pointer_primary_axis < midpoint {
                return slot;
            }

            leading_edge += extent + tab_gap;
            slot += 1;
        }

        slot
    }

    fn tab_drop_slot_from_pointer_primary_axis(
        &self,
        orientation: TabStripOrientation,
        pointer_primary_axis: f32,
    ) -> usize {
        match orientation {
            TabStripOrientation::Horizontal => {
                let scroll_offset_x: f32 =
                    self.tab_strip.horizontal_scroll_handle.offset().x.into();
                let metrics = self.horizontal_tab_metrics();
                Self::tab_drop_slot_from_pointer_primary_axis_for_extents(
                    self.session.tabs.iter().map(|tab| tab.display_width),
                    pointer_primary_axis,
                    scroll_offset_x,
                    metrics.leading_padding,
                    metrics.item_gap,
                )
            }
            TabStripOrientation::Vertical => {
                let scroll_offset_y: f32 = self.tab_strip.vertical_scroll_handle.offset().y.into();
                Self::tab_drop_slot_from_pointer_primary_axis_for_extents(
                    std::iter::repeat_n(SIDEBAR_TAB_ROW_HEIGHT, self.session.tabs.len()),
                    pointer_primary_axis,
                    scroll_offset_y,
                    SIDEBAR_TAB_PADDING_Y,
                    SIDEBAR_TAB_ROW_GAP,
                )
            }
        }
    }

    fn normalized_drop_slot(source_index: usize, raw_slot: usize) -> Option<usize> {
        if raw_slot == source_index || raw_slot == source_index.saturating_add(1) {
            return None;
        }
        Some(raw_slot)
    }

    fn reorder_target_index_for_drop_slot(source_index: usize, drop_slot: usize) -> usize {
        if drop_slot > source_index {
            drop_slot - 1
        } else {
            drop_slot
        }
    }

    fn tab_drop_marker_side_for_slot(index: usize, drop_slot: usize) -> Option<TabDropMarkerSide> {
        if drop_slot == index {
            Some(TabDropMarkerSide::Leading)
        } else if drop_slot == index.saturating_add(1) {
            Some(TabDropMarkerSide::Trailing)
        } else {
            None
        }
    }

    pub(crate) fn tab_drop_marker_side(&self, index: usize) -> Option<TabDropMarkerSide> {
        if index >= self.session.tabs.len() {
            return None;
        }

        let drop_slot = self.tab_strip.drag.and_then(|drag| drag.drop_slot)?;
        Self::tab_drop_marker_side_for_slot(index, drop_slot)
    }

    fn update_tab_drag_marker(
        &mut self,
        orientation: TabStripOrientation,
        pointer_primary_axis: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(source_index) = self.tab_strip.drag.map(|drag| drag.source_index) else {
            return false;
        };

        let raw_drop_slot =
            self.tab_drop_slot_from_pointer_primary_axis(orientation, pointer_primary_axis);
        let next_drop_slot = Self::normalized_drop_slot(source_index, raw_drop_slot);

        let Some(drag) = self.tab_strip.drag.as_mut() else {
            return false;
        };
        if drag.drop_slot == next_drop_slot {
            return false;
        }

        drag.drop_slot = next_drop_slot;
        cx.notify();
        true
    }

    fn auto_scroll_tab_strip_during_drag(
        &mut self,
        orientation: TabStripOrientation,
        pointer_primary_axis: f32,
        viewport_extent: f32,
    ) -> bool {
        if self.tab_strip.drag.is_none() || viewport_extent <= f32::EPSILON {
            return false;
        }

        let edge = TAB_DRAG_AUTOSCROLL_EDGE_WIDTH
            .min(viewport_extent * 0.5)
            .max(f32::EPSILON);
        let leading_strength = ((edge - pointer_primary_axis) / edge).clamp(0.0, 1.0);
        let trailing_start = (viewport_extent - edge).max(0.0);
        let trailing_strength = ((pointer_primary_axis - trailing_start) / edge).clamp(0.0, 1.0);
        let delta = (trailing_strength - leading_strength) * TAB_DRAG_AUTOSCROLL_MAX_STEP;

        match orientation {
            TabStripOrientation::Horizontal => self.scroll_tab_strip_by(-delta),
            TabStripOrientation::Vertical => self.scroll_tab_strip_vertically_by(-delta),
        }
    }

    pub(crate) fn update_tab_drag_preview(
        &mut self,
        orientation: TabStripOrientation,
        pointer_primary_axis: f32,
        viewport_extent: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tab_strip.drag.is_none() {
            return false;
        }
        self.tab_strip
            .drag_preview
            .set_pointer_preview(pointer_primary_axis, viewport_extent);
        let widths_changed = match orientation {
            TabStripOrientation::Horizontal => self.sync_tab_display_widths_for_viewport_if_needed(
                self.tab_strip.drag_preview.viewport_extent(),
            ),
            // Vertical tabs have a fixed row height; no width fitting.
            TabStripOrientation::Vertical => false,
        };

        let scrolled = self.auto_scroll_tab_strip_during_drag(
            orientation,
            pointer_primary_axis,
            viewport_extent,
        );
        let marker_changed = self.update_tab_drag_marker(orientation, pointer_primary_axis, cx);
        if scrolled && !marker_changed {
            cx.notify();
        }
        if widths_changed && !scrolled && !marker_changed {
            cx.notify();
        }
        if scrolled {
            self.ensure_tab_drag_autoscroll_animation(cx);
        } else {
            self.tab_strip.drag_preview.stop_autoscroll_animation();
        }
        scrolled || marker_changed || widths_changed
    }

    pub(crate) fn commit_tab_drag(&mut self, cx: &mut Context<Self>) {
        let drag = self.tab_strip.drag.take();
        self.clear_tab_drag_preview_state();
        let Some(TabDragState {
            source_index,
            drop_slot,
            ..
        }) = drag
        else {
            return;
        };

        let Some(drop_slot) = drop_slot else {
            return;
        };

        let target_index = Self::reorder_target_index_for_drop_slot(source_index, drop_slot);
        if source_index == target_index {
            cx.notify();
            return;
        }

        if !self.reorder_tab(source_index, target_index, cx) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_title_width_px(title: &str) -> f32 {
        title.chars().count() as f32 * 7.0
    }

    #[test]
    fn normalized_drop_slot_filters_noop_boundaries() {
        assert_eq!(TerminalView::normalized_drop_slot(2, 2), None);
        assert_eq!(TerminalView::normalized_drop_slot(2, 3), None);
        assert_eq!(TerminalView::normalized_drop_slot(2, 1), Some(1));
        assert_eq!(TerminalView::normalized_drop_slot(2, 4), Some(4));
    }

    #[test]
    fn reorder_target_index_for_drop_slot_moves_right_correctly() {
        assert_eq!(TerminalView::reorder_target_index_for_drop_slot(1, 3), 2);
        assert_eq!(TerminalView::reorder_target_index_for_drop_slot(0, 3), 2);
    }

    #[test]
    fn reorder_target_index_for_drop_slot_moves_left_correctly() {
        assert_eq!(TerminalView::reorder_target_index_for_drop_slot(3, 1), 1);
        assert_eq!(TerminalView::reorder_target_index_for_drop_slot(2, 0), 0);
    }

    #[test]
    fn horizontal_drop_slot_respects_midpoints() {
        let widths = [100.0, 100.0, 100.0];
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                widths, 40.0, 0.0, 0.0, 0.0,
            ),
            0
        );
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                widths, 70.0, 0.0, 0.0, 0.0,
            ),
            1
        );
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                widths, 170.0, 0.0, 0.0, 0.0,
            ),
            2
        );
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                widths, 270.0, 0.0, 0.0, 0.0,
            ),
            3
        );
    }

    #[test]
    fn horizontal_drop_slot_respects_scroll_offset() {
        let widths = [100.0, 100.0];
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                widths, 40.0, 0.0, 0.0, 0.0,
            ),
            0
        );
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                widths, 40.0, -30.0, 0.0, 0.0,
            ),
            1
        );
    }

    #[test]
    fn drop_slot_respects_leading_inset_and_gap() {
        let extents = [100.0, 100.0];
        // First midpoint sits at inset (6) + 50 = 56.
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                extents, 55.0, 0.0, 6.0, 4.0,
            ),
            0
        );
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                extents, 57.0, 0.0, 6.0, 4.0,
            ),
            1
        );
        // Second midpoint sits at 6 + 100 + 4 + 50 = 160.
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                extents, 161.0, 0.0, 6.0, 4.0,
            ),
            2
        );
    }

    #[test]
    fn tab_drop_marker_side_maps_slot_to_leading_and_trailing_edges() {
        assert_eq!(
            TerminalView::tab_drop_marker_side_for_slot(2, 2),
            Some(TabDropMarkerSide::Leading)
        );
        assert_eq!(
            TerminalView::tab_drop_marker_side_for_slot(2, 3),
            Some(TabDropMarkerSide::Trailing)
        );
        assert_eq!(TerminalView::tab_drop_marker_side_for_slot(2, 1), None);
    }

    #[test]
    fn horizontal_drop_slot_mapping_is_stable_with_adaptive_widths() {
        let effective_max = TerminalView::effective_tab_max_width_for_viewport(1500.0, 3);
        let widths = [
            TerminalView::tab_display_width_for_text_px_with_max(
                synthetic_title_width_px("~/Desktop/claudeCode/claude-code-provider-proxy/docs"),
                effective_max,
            ),
            TerminalView::tab_display_width_for_text_px_with_max(
                synthetic_title_width_px("~"),
                effective_max,
            ),
            TerminalView::tab_display_width_for_text_px_with_max(
                synthetic_title_width_px("~/projects/termy"),
                effective_max,
            ),
        ];

        let first_midpoint = TAB_HORIZONTAL_PADDING + (widths[0] * 0.5);
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                widths,
                first_midpoint - 1.0,
                0.0,
                TAB_HORIZONTAL_PADDING,
                TAB_ITEM_GAP,
            ),
            0
        );
        assert_eq!(
            TerminalView::tab_drop_slot_from_pointer_primary_axis_for_extents(
                widths,
                first_midpoint + 1.0,
                0.0,
                TAB_HORIZONTAL_PADDING,
                TAB_ITEM_GAP,
            ),
            1
        );
    }
}
