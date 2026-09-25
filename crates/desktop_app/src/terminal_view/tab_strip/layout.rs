use super::super::*;
use crate::terminal_view::tab_strip::state::TabStripOrientation;

pub(crate) const TAB_STRIP_RAIL_GUTTER_WIDTH: f32 = 0.0;
const TAB_STRIP_LAYOUT_EPSILON: f32 = 0.001;
#[cfg(target_os = "windows")]
const WINDOWS_CAPTION_BUTTONS_RESERVED_WIDTH: f32 = 140.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TabStripGeometry {
    pub(crate) window_width: f32,
    pub(crate) left_inset_width: f32,
    pub(crate) right_inset_width: f32,
    pub(crate) row_start_x: f32,
    pub(crate) row_end_x: f32,
    pub(crate) row_width: f32,
    pub(crate) tabs_viewport_width: f32,
    pub(crate) gutter_start_x: f32,
    pub(crate) gutter_width: f32,
    pub(crate) action_rail_start_x: f32,
    pub(crate) action_rail_width: f32,
    pub(crate) button_start_x: f32,
    pub(crate) button_end_x: f32,
    pub(crate) button_start_y: f32,
    pub(crate) button_end_y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TabStripLayoutSnapshot {
    pub(crate) geometry: TabStripGeometry,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabStripDragPreview {
    pub(crate) pointer_primary_axis: f32,
    pub(crate) viewport_extent: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabStripLayoutInput {
    pub(crate) viewport_width: f32,
    pub(crate) left_inset_width: f32,
    pub(crate) content_width: Option<f32>,
}

impl TabStripGeometry {
    #[cfg(test)]
    pub(crate) fn left_inset_end_x(self) -> f32 {
        self.row_start_x
    }

    pub(crate) fn tabs_viewport_end_x(self) -> f32 {
        self.row_start_x + self.tabs_viewport_width
    }

    pub(crate) fn gutter_end_x(self) -> f32 {
        self.gutter_start_x + self.gutter_width
    }

    pub(crate) fn action_rail_end_x(self) -> f32 {
        self.action_rail_start_x + self.action_rail_width
    }

    #[cfg(test)]
    pub(crate) fn right_inset_start_x(self) -> f32 {
        self.row_end_x
    }

    pub(crate) fn contains_tabs_viewport_x(self, x: f32) -> bool {
        x >= self.row_start_x && x < self.tabs_viewport_end_x()
    }

    #[cfg(test)]
    pub(crate) fn contains_gutter_x(self, x: f32) -> bool {
        x >= self.gutter_start_x && x < self.gutter_end_x()
    }

    pub(crate) fn contains_action_rail_x(self, x: f32) -> bool {
        x >= self.action_rail_start_x && x < self.action_rail_end_x()
    }

    pub(crate) fn new_tab_button_contains(self, x: f32, y: f32) -> bool {
        x >= self.button_start_x
            && x < self.button_end_x
            && y >= self.button_start_y
            && y < self.button_end_y
    }
}

impl TerminalView {
    fn horizontal_action_rail_width_for_platform(max_row_width: f32) -> f32 {
        TABBAR_ACTION_RAIL_WIDTH.min(max_row_width)
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn titlebar_right_padding_for_platform() -> f32 {
        WINDOWS_CAPTION_BUTTONS_RESERVED_WIDTH
    }

    #[cfg(not(target_os = "windows"))]
    pub(crate) fn titlebar_right_padding_for_platform() -> f32 {
        TOP_STRIP_SIDE_PADDING
    }

    pub(crate) fn titlebar_left_padding_for_platform() -> f32 {
        if cfg!(target_os = "macos") {
            TOP_STRIP_MACOS_TRAFFIC_LIGHT_PADDING
        } else {
            TOP_STRIP_SIDE_PADDING
        }
    }

    pub(crate) fn tab_strip_layout_for_input(input: TabStripLayoutInput) -> TabStripLayoutSnapshot {
        let window_width = input.viewport_width.max(0.0);
        let left_inset_width = input.left_inset_width.max(0.0).min(window_width);
        let remaining_after_left = (window_width - left_inset_width).max(0.0);
        let reserved_right_inset_width =
            Self::titlebar_right_padding_for_platform().min(remaining_after_left);
        let max_row_width = (remaining_after_left - reserved_right_inset_width).max(0.0);
        let action_rail_width = Self::horizontal_action_rail_width_for_platform(max_row_width);
        let available_after_rail = (max_row_width - action_rail_width).max(0.0);
        let gutter_width = if action_rail_width > f32::EPSILON {
            TAB_STRIP_RAIL_GUTTER_WIDTH.min(available_after_rail)
        } else {
            0.0
        };
        let max_tabs_viewport_width = (max_row_width - action_rail_width - gutter_width).max(0.0);
        let tabs_viewport_width = input
            .content_width
            .map_or(max_tabs_viewport_width, |width| {
                width.max(0.0).min(max_tabs_viewport_width)
            });
        let row_width = (tabs_viewport_width + gutter_width + action_rail_width)
            .min(remaining_after_left)
            .max(0.0);
        let right_inset_width = (remaining_after_left - row_width).max(0.0);
        let row_start_x = left_inset_width;
        let row_end_x = row_start_x + row_width;
        let gutter_start_x = row_start_x + tabs_viewport_width;
        let action_rail_start_x = gutter_start_x + gutter_width;
        let button_size = TABBAR_NEW_TAB_BUTTON_SIZE.min(action_rail_width);
        let button_start_x = action_rail_start_x;
        let button_start_y =
            TOP_STRIP_CONTENT_OFFSET_Y + ((TABBAR_HEIGHT - button_size) * 0.5).max(0.0);
        let button_end_x = button_start_x + button_size;
        let button_end_y = button_start_y + button_size;

        let geometry = TabStripGeometry {
            window_width,
            left_inset_width,
            right_inset_width,
            row_start_x,
            row_end_x,
            row_width,
            tabs_viewport_width,
            gutter_start_x,
            gutter_width,
            action_rail_start_x,
            action_rail_width,
            button_start_x,
            button_end_x,
            button_start_y,
            button_end_y,
        };

        debug_assert!(
            (geometry.left_inset_width + geometry.row_width + geometry.right_inset_width
                - geometry.window_width)
                .abs()
                <= TAB_STRIP_LAYOUT_EPSILON
        );
        debug_assert!(
            geometry.tabs_viewport_end_x() <= geometry.gutter_start_x + TAB_STRIP_LAYOUT_EPSILON
        );
        debug_assert!(
            geometry.gutter_end_x() <= geometry.action_rail_start_x + TAB_STRIP_LAYOUT_EPSILON
        );
        debug_assert!(
            geometry.action_rail_end_x() <= geometry.row_end_x + TAB_STRIP_LAYOUT_EPSILON
        );
        debug_assert!(geometry.row_end_x <= geometry.window_width + TAB_STRIP_LAYOUT_EPSILON);
        debug_assert!(
            (geometry.row_end_x + geometry.right_inset_width - geometry.window_width).abs()
                <= TAB_STRIP_LAYOUT_EPSILON
        );
        debug_assert!(geometry.action_rail_start_x <= geometry.action_rail_end_x());
        debug_assert!(geometry.button_start_x >= geometry.action_rail_start_x);
        debug_assert!(geometry.button_end_x <= geometry.action_rail_end_x() + f32::EPSILON);

        TabStripLayoutSnapshot { geometry }
    }

    #[cfg(test)]
    pub(crate) fn tab_strip_layout_for_viewport_width(
        viewport_width: f32,
    ) -> TabStripLayoutSnapshot {
        Self::tab_strip_layout_for_viewport_with_left_inset(
            viewport_width,
            Self::titlebar_left_padding_for_platform(),
        )
    }

    pub(crate) fn tab_strip_layout_for_viewport_with_left_inset(
        viewport_width: f32,
        left_inset_width: f32,
    ) -> TabStripLayoutSnapshot {
        Self::tab_strip_layout_for_input(TabStripLayoutInput {
            viewport_width,
            left_inset_width,
            content_width: None,
        })
    }

    // Windows keeps the action rail directly after the tabs so the remaining
    // titlebar space stays available for window dragging before the native
    // caption buttons. Other platforms keep the rail right-anchored.
    #[cfg(any(test, target_os = "windows"))]
    pub(crate) fn tab_strip_layout_for_viewport_with_left_inset_and_content_width(
        viewport_width: f32,
        left_inset_width: f32,
        content_width: f32,
    ) -> TabStripLayoutSnapshot {
        Self::tab_strip_layout_for_input(TabStripLayoutInput {
            viewport_width,
            left_inset_width,
            content_width: Some(content_width),
        })
    }

    /// Left inset before the first tab. The workspace sidebar replaces the
    /// platform titlebar padding when visible so the tab strip starts flush
    /// with the sidebar's right edge.
    pub(crate) fn tab_strip_base_left_inset(&self) -> f32 {
        let workspace_sidebar_width = self.workspace_sidebar_width();
        if workspace_sidebar_width > 0.0 {
            workspace_sidebar_width
        } else {
            Self::titlebar_left_padding_for_platform()
        }
    }

    pub(crate) fn tab_strip_layout(&self, window: &Window) -> TabStripLayoutSnapshot {
        let viewport_width: f32 = window.viewport_size().width.into();
        Self::tab_strip_layout_for_viewport_with_left_inset(
            viewport_width,
            self.tab_strip_base_left_inset(),
        )
    }

    pub(crate) fn tab_strip_layout_snapshot(&self) -> Option<TabStripLayoutSnapshot> {
        self.tab_strip.horizontal_layout_snapshot
    }

    pub(crate) fn tab_strip_layout_snapshot_or_window(
        &self,
        window: &Window,
    ) -> TabStripLayoutSnapshot {
        self.tab_strip_layout_snapshot()
            .unwrap_or_else(|| self.tab_strip_layout(window))
    }

    pub(crate) fn set_tab_strip_layout_snapshot(&mut self, snapshot: TabStripLayoutSnapshot) {
        self.tab_strip.horizontal_layout_snapshot = Some(snapshot);
    }

    #[cfg(test)]
    pub(crate) fn tab_strip_geometry_for_viewport_width(viewport_width: f32) -> TabStripGeometry {
        Self::tab_strip_layout_for_viewport_width(viewport_width).geometry
    }

    #[cfg(test)]
    pub(crate) fn tab_strip_geometry_for_viewport_with_left_inset(
        viewport_width: f32,
        left_inset_width: f32,
    ) -> TabStripGeometry {
        Self::tab_strip_layout_for_viewport_with_left_inset(viewport_width, left_inset_width)
            .geometry
    }

    pub(crate) fn tab_strip_geometry(&self, window: &Window) -> TabStripGeometry {
        self.tab_strip_layout_snapshot_or_window(window).geometry
    }

    pub(crate) fn tab_strip_pointer_x_from_window_x_for_geometry(
        window_x: f32,
        geometry: TabStripGeometry,
    ) -> f32 {
        (window_x - geometry.row_start_x).clamp(0.0, geometry.tabs_viewport_width)
    }

    pub(crate) fn tab_strip_pointer_x_from_window_x(
        &self,
        window: &Window,
        window_x: Pixels,
    ) -> (f32, f32) {
        let geometry = self.tab_strip_geometry(window);
        let pointer_x =
            Self::tab_strip_pointer_x_from_window_x_for_geometry(window_x.into(), geometry);
        (pointer_x, geometry.tabs_viewport_width)
    }

    pub(crate) fn tab_strip_drag_preview(
        &self,
        orientation: TabStripOrientation,
        window: &Window,
        position: gpui::Point<Pixels>,
    ) -> TabStripDragPreview {
        match orientation {
            TabStripOrientation::Horizontal => {
                let (pointer_primary_axis, viewport_extent) =
                    self.tab_strip_pointer_x_from_window_x(window, position.x);
                TabStripDragPreview {
                    pointer_primary_axis,
                    viewport_extent,
                }
            }
            TabStripOrientation::Vertical => {
                // The sidebar's scrollable tab list begins below the titlebar +
                // sidebar header; map the window Y into that content space (the
                // drop-slot helper adds the scroll offset itself).
                let window_y: f32 = position.y.into();
                let tabs_top = self.terminal_content_top_inset() + SIDEBAR_HEADER_HEIGHT;
                TabStripDragPreview {
                    pointer_primary_axis: window_y - tabs_top,
                    viewport_extent: self
                        .tab_strip
                        .vertical_layout_last_synced_viewport_height
                        .max(0.0),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_float_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.0001,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn viewport_and_rail_never_overlap() {
        let snapshot = TerminalView::tab_strip_layout_for_viewport_width(1280.0);
        let geometry = snapshot.geometry;
        assert!(geometry.tabs_viewport_end_x() <= geometry.gutter_start_x);
        assert!(geometry.gutter_end_x() <= geometry.action_rail_start_x);
        assert!(geometry.tabs_viewport_end_x() <= geometry.action_rail_start_x);
    }

    #[test]
    fn layout_contract_covers_full_window_width() {
        let snapshot = TerminalView::tab_strip_layout_for_viewport_width(1280.0);
        let geometry = snapshot.geometry;
        assert_float_eq(
            geometry.left_inset_width
                + geometry.tabs_viewport_width
                + geometry.gutter_width
                + geometry.action_rail_width
                + geometry.right_inset_width,
            geometry.window_width,
        );
    }

    #[test]
    fn lane_boundaries_are_continuous_without_overlap() {
        let snapshot = TerminalView::tab_strip_layout_for_viewport_width(1280.0);
        let geometry = snapshot.geometry;
        assert_float_eq(geometry.left_inset_end_x(), geometry.row_start_x);
        assert_float_eq(geometry.tabs_viewport_end_x(), geometry.gutter_start_x);
        assert_float_eq(geometry.gutter_end_x(), geometry.action_rail_start_x);
        assert_float_eq(geometry.action_rail_end_x(), geometry.row_end_x);
        assert_float_eq(geometry.right_inset_start_x(), geometry.row_end_x);
        assert_float_eq(
            geometry.row_end_x + geometry.right_inset_width,
            geometry.window_width,
        );
    }

    #[test]
    fn button_is_always_inside_rail() {
        let snapshot = TerminalView::tab_strip_layout_for_viewport_width(960.0);
        let geometry = snapshot.geometry;
        assert!(geometry.button_start_x >= geometry.action_rail_start_x);
        assert!(geometry.button_end_x <= geometry.action_rail_end_x());
    }

    #[test]
    fn button_shrinks_to_narrow_action_rail() {
        let viewport_width = TerminalView::titlebar_left_padding_for_platform()
            + TerminalView::titlebar_right_padding_for_platform()
            + 8.0;
        let snapshot = TerminalView::tab_strip_layout_for_viewport_width(viewport_width);
        let geometry = snapshot.geometry;
        assert_float_eq(geometry.action_rail_width, 8.0);
        assert_float_eq(
            geometry.button_end_x - geometry.button_start_x,
            geometry.action_rail_width,
        );
        assert!(geometry.button_start_x >= geometry.action_rail_start_x);
        assert!(geometry.button_end_x <= geometry.action_rail_end_x());
    }

    #[test]
    fn action_rail_keeps_positive_width_in_standard_viewport_on_every_platform() {
        let snapshot = TerminalView::tab_strip_layout_for_viewport_width(1280.0);
        assert!(snapshot.geometry.action_rail_width > 0.0);
    }

    #[test]
    fn button_stays_inside_window_bounds() {
        let snapshot = TerminalView::tab_strip_layout_for_viewport_width(1280.0);
        let geometry = snapshot.geometry;
        assert!(geometry.button_start_x >= geometry.action_rail_start_x);
        assert!(geometry.button_end_x <= geometry.action_rail_end_x());
        assert!(geometry.button_end_x <= geometry.window_width - geometry.right_inset_width);
    }

    #[test]
    fn gutter_divider_is_present_and_fixed() {
        let snapshot = TerminalView::tab_strip_layout_for_viewport_width(1280.0);
        assert_float_eq(snapshot.geometry.gutter_width, TAB_STRIP_RAIL_GUTTER_WIDTH);
    }

    #[test]
    fn baseline_segments_are_continuous_across_row_contract() {
        let snapshot = TerminalView::tab_strip_layout_for_viewport_width(1280.0);
        let geometry = snapshot.geometry;
        assert_float_eq(
            geometry.row_start_x + geometry.row_width,
            geometry.row_end_x,
        );
        assert_float_eq(geometry.action_rail_end_x(), geometry.row_end_x);
        assert_float_eq(
            geometry.tabs_viewport_width + geometry.gutter_width + geometry.action_rail_width,
            geometry.row_width,
        );
    }

    #[test]
    fn custom_left_inset_keeps_lane_contract_intact() {
        let snapshot = TerminalView::tab_strip_layout_for_viewport_with_left_inset(1280.0, 132.0);
        let geometry = snapshot.geometry;
        assert_float_eq(geometry.left_inset_width, 132.0);
        assert_float_eq(geometry.left_inset_end_x(), geometry.row_start_x);
        assert_float_eq(geometry.tabs_viewport_end_x(), geometry.gutter_start_x);
        assert_float_eq(geometry.gutter_end_x(), geometry.action_rail_start_x);
        assert_float_eq(geometry.action_rail_end_x(), geometry.row_end_x);
        assert_float_eq(
            geometry.row_end_x + geometry.right_inset_width,
            geometry.window_width,
        );
    }

    #[test]
    fn fitting_content_places_action_rail_before_trailing_drag_region() {
        let left_inset_width = TerminalView::titlebar_left_padding_for_platform();
        let content_width = 192.0;
        let snapshot =
            TerminalView::tab_strip_layout_for_viewport_with_left_inset_and_content_width(
                1280.0,
                left_inset_width,
                content_width,
            );
        let geometry = snapshot.geometry;

        assert_float_eq(geometry.tabs_viewport_width, content_width);
        assert_float_eq(geometry.action_rail_width, TABBAR_ACTION_RAIL_WIDTH);
        assert_float_eq(geometry.gutter_width, TAB_STRIP_RAIL_GUTTER_WIDTH);
        assert!(
            geometry.right_inset_width > TerminalView::titlebar_right_padding_for_platform(),
            "expected leftover titlebar slack to move into the trailing drag region",
        );
        assert_float_eq(
            geometry.row_end_x,
            geometry.row_start_x
                + content_width
                + TAB_STRIP_RAIL_GUTTER_WIDTH
                + TABBAR_ACTION_RAIL_WIDTH,
        );
    }
}
