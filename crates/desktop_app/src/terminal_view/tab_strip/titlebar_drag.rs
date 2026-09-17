use super::super::*;
use super::state::TabStripOrientation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HorizontalTitlebarPointerTarget {
    NativeCaptionButtons,
    InteractiveChrome,
    DragSurface,
}

impl TerminalView {
    pub(crate) fn disarm_titlebar_window_move(&mut self) {
        self.tab_strip.titlebar.disarm();
    }

    #[cfg(target_os = "windows")]
    fn horizontal_native_caption_buttons_hit_test_for_geometry(
        x: f32,
        geometry: crate::terminal_view::tab_strip::layout::TabStripGeometry,
    ) -> bool {
        let caption_width = geometry
            .right_inset_width
            .min(Self::titlebar_right_padding_for_platform());
        caption_width > f32::EPSILON
            && x >= geometry.window_width - caption_width
            && x < geometry.window_width
    }

    #[cfg(not(target_os = "windows"))]
    fn horizontal_native_caption_buttons_hit_test_for_geometry(
        x: f32,
        geometry: crate::terminal_view::tab_strip::layout::TabStripGeometry,
    ) -> bool {
        let _ = (x, geometry);
        false
    }

    fn horizontal_titlebar_pointer_target_for_geometry(
        x: f32,
        y: f32,
        show_tab_strip_chrome: bool,
        geometry: crate::terminal_view::tab_strip::layout::TabStripGeometry,
        tab_widths: impl IntoIterator<Item = f32>,
        scroll_offset_x: f32,
    ) -> HorizontalTitlebarPointerTarget {
        if Self::horizontal_native_caption_buttons_hit_test_for_geometry(x, geometry) {
            return HorizontalTitlebarPointerTarget::NativeCaptionButtons;
        }

        if Self::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
            x,
            y,
            show_tab_strip_chrome,
            geometry,
            tab_widths,
            scroll_offset_x,
        ) {
            HorizontalTitlebarPointerTarget::InteractiveChrome
        } else {
            HorizontalTitlebarPointerTarget::DragSurface
        }
    }

    fn handle_window_drag_surface_mouse_down(
        &mut self,
        orientation: TabStripOrientation,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }

        let x: f32 = event.position.x.into();
        let y: f32 = event.position.y.into();
        let interactive_hit = match orientation {
            TabStripOrientation::Horizontal => {
                if self.tab_strip.drag.is_some() {
                    self.disarm_titlebar_window_move();
                    return;
                }
                let geometry = self.tab_strip_geometry(window);
                let scroll_offset_x: f32 =
                    self.tab_strip.horizontal_scroll_handle.offset().x.into();
                match Self::horizontal_titlebar_pointer_target_for_geometry(
                    x,
                    y,
                    self.should_render_tab_strip_chrome(),
                    geometry,
                    self.session.tabs.iter().map(|tab| tab.display_width),
                    scroll_offset_x,
                ) {
                    HorizontalTitlebarPointerTarget::NativeCaptionButtons => {
                        self.disarm_titlebar_window_move();
                        return;
                    }
                    HorizontalTitlebarPointerTarget::InteractiveChrome => true,
                    HorizontalTitlebarPointerTarget::DragSurface => false,
                }
            }
            // In vertical/sidebar mode the titlebar holds no tabs, so the whole
            // top strip is a window-drag surface. (This handler is only ever
            // invoked with Horizontal today; arm kept for exhaustiveness.)
            TabStripOrientation::Vertical => false,
        };
        let outcome = self
            .tab_strip
            .titlebar
            .on_mouse_down(interactive_hit, event.click_count);
        if outcome.trigger_window_action {
            #[cfg(target_os = "macos")]
            window.titlebar_double_click();
            #[cfg(not(target_os = "macos"))]
            window.zoom_window();
            window.prevent_default();
            cx.stop_propagation();
            return;
        }

        if interactive_hit {
            self.disarm_titlebar_window_move();
            window.prevent_default();
            cx.stop_propagation();
            return;
        }

        if outcome.arm_move {
            #[cfg(target_os = "macos")]
            {
                // AppKit needs the original mouse-down event, not a later
                // mouse-move. It may consume mouse-up, so clear our arm now.
                self.disarm_titlebar_window_move();
                if let Err(error) = crate::macos_titlebar_drag::start_titlebar_window_drag(window) {
                    log::warn!("Failed to start native titlebar drag: {error}");
                }
            }
            window.prevent_default();
            cx.stop_propagation();
        }
    }

    pub(crate) fn handle_unified_titlebar_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_window_drag_surface_mouse_down(
            TabStripOrientation::Horizontal,
            event,
            window,
            cx,
        );
    }

    pub(crate) fn handle_unified_titlebar_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }

        let x: f32 = event.position.x.into();
        let geometry = self.tab_strip_geometry(window);
        if Self::horizontal_native_caption_buttons_hit_test_for_geometry(x, geometry) {
            self.disarm_titlebar_window_move();
            return;
        }

        self.tab_strip.titlebar.on_mouse_up();
        window.prevent_default();
        cx.stop_propagation();
    }

    pub(crate) fn maybe_start_titlebar_window_move(
        &mut self,
        dragging: bool,
        window: &mut Window,
    ) -> bool {
        if !self
            .tab_strip
            .titlebar
            .take_window_move_request(dragging, self.tab_strip.drag.is_some())
        {
            return false;
        }

        window.start_window_move();
        true
    }

    fn handle_window_drag_surface_mouse_move(
        &mut self,
        orientation: TabStripOrientation,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab_strip.drag.is_some() {
            self.disarm_titlebar_window_move();

            let changed = self.clear_tab_hover_state();
            if event.dragging() {
                let preview = self.tab_strip_drag_preview(orientation, window, event.position);
                if !self.update_tab_drag_preview(
                    orientation,
                    preview.pointer_primary_axis,
                    preview.viewport_extent,
                    cx,
                ) && changed
                {
                    cx.notify();
                }
            } else {
                self.commit_tab_drag(cx);
                if changed {
                    cx.notify();
                }
            }

            cx.stop_propagation();
            return;
        }

        if self.maybe_start_titlebar_window_move(event.dragging(), window) {
            cx.stop_propagation();
            return;
        }

        let mut changed = false;
        if self.clear_tab_hover_state() {
            changed = true;
        }
        if event.dragging() {
            let preview = self.tab_strip_drag_preview(orientation, window, event.position);
            if !self.update_tab_drag_preview(
                orientation,
                preview.pointer_primary_axis,
                preview.viewport_extent,
                cx,
            ) && changed
            {
                cx.notify();
            }
            return;
        }
        if self.tab_strip.drag.is_some() {
            self.commit_tab_drag(cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn handle_titlebar_tab_strip_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_window_drag_surface_mouse_move(
            TabStripOrientation::Horizontal,
            event,
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    use crate::terminal_view::tab_strip::layout::TabStripGeometry;
    use crate::terminal_view::tab_strip::state::TabStripTitlebarState;

    fn armed_titlebar_state() -> TabStripTitlebarState {
        let mut state = TabStripTitlebarState::default();
        let outcome = state.on_mouse_down(false, 1);
        assert!(outcome.arm_move);
        state
    }

    fn hidden_titlebar_interactive_hit() -> bool {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        let x = geometry.row_start_x + TAB_HORIZONTAL_PADDING + 12.0;
        let y = TOP_STRIP_CONTENT_OFFSET_Y + TABBAR_HEIGHT - 1.0;

        TerminalView::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
            x,
            y,
            false,
            geometry,
            [120.0],
            0.0,
        )
    }

    #[cfg(target_os = "windows")]
    fn windows_horizontal_geometry_with_extra_slack() -> TabStripGeometry {
        TerminalView::tab_strip_layout_for_viewport_with_left_inset_and_content_width(
            1280.0,
            TerminalView::titlebar_left_padding_for_platform(),
            192.0,
        )
        .geometry
    }

    #[test]
    fn titlebar_window_move_requires_armed_and_dragging() {
        assert!(!TabStripTitlebarState::default().should_start_window_move(true, false));
        assert!(!armed_titlebar_state().should_start_window_move(false, false));
        assert!(armed_titlebar_state().should_start_window_move(true, false));
    }

    #[test]
    fn titlebar_window_move_does_not_start_during_tab_drag() {
        assert!(!armed_titlebar_state().should_start_window_move(true, true));
    }

    #[test]
    fn hidden_horizontal_titlebar_arms_window_move() {
        let interactive = hidden_titlebar_interactive_hit();
        let mut state = TabStripTitlebarState::default();

        assert!(!interactive);
        assert!(state.on_mouse_down(interactive, 1).arm_move);
    }

    #[test]
    fn hidden_titlebar_double_click_uses_titlebar_double_click_branch() {
        let interactive = hidden_titlebar_interactive_hit();
        let mut state = TabStripTitlebarState::default();

        assert!(!interactive);
        let outcome = state.on_mouse_down(interactive, 2);
        assert!(!outcome.arm_move);
        assert!(outcome.trigger_window_action);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_caption_hit_test_uses_only_reserved_trailing_slice() {
        let geometry = windows_horizontal_geometry_with_extra_slack();
        let caption_width = geometry
            .right_inset_width
            .min(TerminalView::titlebar_right_padding_for_platform());
        let caption_start = geometry.window_width - caption_width;

        assert!(caption_start > geometry.row_end_x);
        assert!(
            !TerminalView::horizontal_native_caption_buttons_hit_test_for_geometry(
                geometry.row_end_x + 8.0,
                geometry,
            )
        );
        assert!(
            TerminalView::horizontal_native_caption_buttons_hit_test_for_geometry(
                caption_start + 8.0,
                geometry,
            )
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_caption_buttons_pass_through_even_when_tab_strip_is_hidden() {
        let geometry = windows_horizontal_geometry_with_extra_slack();
        let caption_width = geometry
            .right_inset_width
            .min(TerminalView::titlebar_right_padding_for_platform());
        let x = geometry.window_width - (caption_width * 0.5);

        assert_eq!(
            TerminalView::horizontal_titlebar_pointer_target_for_geometry(
                x,
                TOP_STRIP_CONTENT_OFFSET_Y + 4.0,
                false,
                geometry,
                [120.0],
                0.0,
            ),
            HorizontalTitlebarPointerTarget::NativeCaptionButtons
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_action_rail_before_trailing_slack_stays_interactive() {
        let geometry = windows_horizontal_geometry_with_extra_slack();
        let x = (geometry.button_start_x + geometry.button_end_x) * 0.5;
        let y = (geometry.button_start_y + geometry.button_end_y) * 0.5;

        assert!(geometry.action_rail_width > 0.0);
        assert!(geometry.right_inset_width > TerminalView::titlebar_right_padding_for_platform());
        assert_eq!(
            TerminalView::horizontal_titlebar_pointer_target_for_geometry(
                x,
                y,
                true,
                geometry,
                [120.0],
                0.0,
            ),
            HorizontalTitlebarPointerTarget::InteractiveChrome
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_extra_right_inset_slack_stays_draggable() {
        let geometry = windows_horizontal_geometry_with_extra_slack();
        let caption_width = geometry
            .right_inset_width
            .min(TerminalView::titlebar_right_padding_for_platform());
        let x = (geometry.row_end_x + (geometry.window_width - caption_width)) * 0.5;

        assert_eq!(
            TerminalView::horizontal_titlebar_pointer_target_for_geometry(
                x,
                TOP_STRIP_CONTENT_OFFSET_Y + 4.0,
                true,
                geometry,
                [120.0],
                0.0,
            ),
            HorizontalTitlebarPointerTarget::DragSurface
        );
    }
}
