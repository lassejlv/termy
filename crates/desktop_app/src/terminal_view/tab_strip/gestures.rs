use super::super::*;
use super::state::TabStripOrientation;

impl TerminalView {
    pub(crate) fn tab_strip_scroll_delta_from_pixels(delta_x: f32, delta_y: f32) -> f32 {
        if delta_x.abs() <= f32::EPSILON && delta_y.abs() <= f32::EPSILON {
            return 0.0;
        }

        let dominant_delta = if delta_x.abs() >= delta_y.abs() {
            delta_x
        } else {
            delta_y
        };

        // ScrollHandle offset-space is [-max, 0], while input deltas are content-space.
        // Invert once here so all callers can pass the resulting offset delta directly.
        -dominant_delta
    }

    pub(crate) fn handle_tab_strip_action_rail_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pixel_delta = event
            .delta
            .pixel_delta(px(TAB_STRIP_WHEEL_DELTA_LINE_REFERENCE_PX));
        let delta_x: f32 = pixel_delta.x.into();
        let delta_y: f32 = pixel_delta.y.into();
        let scroll_delta = Self::tab_strip_scroll_delta_from_pixels(delta_x, delta_y);
        if self.scroll_tab_strip_by(scroll_delta) {
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(crate) fn on_tab_mouse_down(
        &mut self,
        orientation: TabStripOrientation,
        tab_index: usize,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.disarm_titlebar_window_move();
        self.switch_tab(tab_index, cx);
        self.focus_terminal_after_tab_activation(window, cx);
        self.begin_tab_drag(tab_index, orientation);
        if click_count != 2 {
            self.arm_window_tab_drag(tab_index, window.mouse_position(), cx);
        }
        if Self::should_begin_tab_rename(orientation, click_count) {
            self.begin_rename_tab(tab_index, cx);
        }
    }

    fn should_begin_tab_rename(_orientation: TabStripOrientation, click_count: usize) -> bool {
        click_count == 2
    }

    fn set_hovered_tab(&mut self, tab_index: usize, cx: &mut Context<Self>) -> bool {
        if self.tab_strip.hovered_tab == Some(tab_index) {
            return false;
        }
        let now = Instant::now();
        if let Some(previous) = self.tab_strip.hovered_tab {
            self.tab_strip.start_unhover_animation(previous, now);
        }
        self.tab_strip.hovered_tab = Some(tab_index);
        self.tab_strip.start_hover_animation(tab_index, now);
        self.schedule_tab_interaction_animation(cx);
        true
    }

    // Clears tab hover with a fade-out, unlike the instant clear_tab_hover_state
    // used by hard interaction resets.
    fn clear_tab_hover_state_animated(&mut self, cx: &mut Context<Self>) -> bool {
        let previous = self.tab_strip.hovered_tab;
        let changed = self.clear_tab_hover_state();
        if let Some(previous) = previous {
            self.tab_strip
                .start_unhover_animation(previous, Instant::now());
            self.schedule_tab_interaction_animation(cx);
        }
        changed
    }

    pub(crate) fn on_tab_close_mouse_move(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        let mut hover_changed = self.set_hovered_tab(tab_index, cx);
        if self.tab_strip.hovered_tab_close != Some(tab_index) {
            self.tab_strip.hovered_tab_close = Some(tab_index);
            hover_changed = true;
        }
        if hover_changed {
            cx.notify();
        }
    }

    pub(crate) fn on_tab_mouse_move(
        &mut self,
        orientation: TabStripOrientation,
        tab_index: usize,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let mut hovered_changed = self.set_hovered_tab(tab_index, cx);
        if self.tab_strip.hovered_tab_close.take().is_some() {
            hovered_changed = true;
        }

        let drag_changed = if event.dragging() {
            self.disarm_titlebar_window_move();
            let preview = self.tab_strip_drag_preview(orientation, window, event.position);
            self.update_tab_drag_preview(
                orientation,
                preview.pointer_primary_axis,
                preview.viewport_extent,
                cx,
            )
        } else {
            false
        };
        if hovered_changed && !drag_changed {
            cx.notify();
        }
    }

    pub(crate) fn on_tabs_content_mouse_move(
        &mut self,
        orientation: TabStripOrientation,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let hovered_changed = self.clear_tab_hover_state_animated(cx);
        let drag_changed = if event.dragging() {
            if self.tab_strip.drag.is_some() {
                self.disarm_titlebar_window_move();
            }
            let preview = self.tab_strip_drag_preview(orientation, window, event.position);
            self.update_tab_drag_preview(
                orientation,
                preview.pointer_primary_axis,
                preview.viewport_extent,
                cx,
            )
        } else {
            if self.tab_strip.drag.is_some() {
                self.commit_tab_drag(cx);
                return;
            }
            false
        };
        if hovered_changed && !drag_changed {
            cx.notify();
        }
    }

    pub(crate) fn on_action_rail_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let hovered_changed = self.clear_tab_hover_state_animated(cx);
        if !event.dragging() {
            if hovered_changed {
                cx.notify();
            }
            return;
        }

        if self.tab_strip.drag.is_some() {
            self.disarm_titlebar_window_move();
        }
        let preview =
            self.tab_strip_drag_preview(TabStripOrientation::Horizontal, window, event.position);
        if !self.update_tab_drag_preview(
            TabStripOrientation::Horizontal,
            preview.pointer_primary_axis,
            preview.viewport_extent,
            cx,
        ) && hovered_changed
        {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_strip_scroll_delta_prefers_horizontal_axis() {
        assert_eq!(
            TerminalView::tab_strip_scroll_delta_from_pixels(48.0, 12.0),
            -48.0
        );
    }

    #[test]
    fn tab_strip_scroll_delta_prefers_vertical_axis_when_dominant() {
        assert_eq!(
            TerminalView::tab_strip_scroll_delta_from_pixels(12.0, 48.0),
            -48.0
        );
    }

    #[test]
    fn tab_strip_scroll_delta_preserves_small_non_zero_trackpad_deltas() {
        assert_eq!(
            TerminalView::tab_strip_scroll_delta_from_pixels(0.2, -0.4),
            0.4
        );
    }

    #[test]
    fn tab_strip_scroll_delta_returns_zero_only_for_zero_input() {
        assert_eq!(
            TerminalView::tab_strip_scroll_delta_from_pixels(0.0, 0.0),
            0.0
        );
    }

    #[test]
    fn tab_strip_scroll_delta_falls_back_to_vertical_axis_for_zero_horizontal() {
        assert_eq!(
            TerminalView::tab_strip_scroll_delta_from_pixels(0.0, -30.0),
            30.0
        );
    }

    #[test]
    fn tab_strip_scroll_delta_falls_back_to_horizontal_axis_for_zero_vertical() {
        assert_eq!(
            TerminalView::tab_strip_scroll_delta_from_pixels(12.0, 0.0),
            -12.0
        );
    }
}
