use super::super::*;
use super::layout::TabStripGeometry;

impl TerminalView {
    /// Termy-style spacing; the strip itself passes its geometry's spacing.
    #[cfg(test)]
    pub(crate) fn unified_titlebar_tab_shell_hit_test(
        pointer_x: f32,
        pointer_y: f32,
        tab_widths: impl IntoIterator<Item = f32>,
        scroll_offset_x: f32,
    ) -> bool {
        Self::unified_titlebar_tab_shell_hit_test_with_spacing(
            pointer_x,
            pointer_y,
            tab_widths,
            scroll_offset_x,
            TAB_HORIZONTAL_PADDING,
            TAB_ITEM_GAP,
        )
    }

    pub(crate) fn unified_titlebar_tab_shell_hit_test_with_spacing(
        pointer_x: f32,
        pointer_y: f32,
        tab_widths: impl IntoIterator<Item = f32>,
        scroll_offset_x: f32,
        leading_padding: f32,
        item_gap: f32,
    ) -> bool {
        let tab_top = TOP_STRIP_CONTENT_OFFSET_Y;
        let tab_bottom = TOP_STRIP_CONTENT_OFFSET_Y + TABBAR_HEIGHT;
        if pointer_y < tab_top || pointer_y >= tab_bottom {
            return false;
        }

        let mut left = leading_padding + scroll_offset_x;
        for width in tab_widths {
            let right = left + width;
            if pointer_x >= left && pointer_x < right {
                return true;
            }
            left = right + item_gap;
        }

        false
    }

    pub(crate) fn unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
        x: f32,
        y: f32,
        show_tab_strip_chrome: bool,
        geometry: TabStripGeometry,
        tab_widths: impl IntoIterator<Item = f32>,
        scroll_offset_x: f32,
    ) -> bool {
        // Hidden-titlebar states still render top padding/branding, but no tab-strip controls.
        // Treat that entire surface as noninteractive so it can arm window dragging instead of
        // reusing invisible tab geometry from the collapsed strip layout.
        if !show_tab_strip_chrome {
            return false;
        }

        if geometry.contains_tabs_viewport_x(x) {
            let pointer_x = (x - geometry.row_start_x).clamp(0.0, geometry.tabs_viewport_width);
            if Self::unified_titlebar_tab_shell_hit_test_with_spacing(
                pointer_x,
                y,
                tab_widths,
                scroll_offset_x,
                geometry.tab_leading_padding,
                geometry.tab_item_gap,
            ) {
                return true;
            }
        }

        if !geometry.contains_action_rail_x(x) {
            return false;
        }

        geometry.new_tab_button_contains(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_hit_test_y() -> f32 {
        TOP_STRIP_CONTENT_OFFSET_Y + TABBAR_HEIGHT - 1.0
    }

    #[test]
    fn shell_hit_test_detects_tabs_and_respects_y_bounds() {
        let widths = [100.0, 120.0];
        let scroll_offset_x = 0.0;
        let tab_top = TOP_STRIP_CONTENT_OFFSET_Y;
        let tab_bottom = TOP_STRIP_CONTENT_OFFSET_Y + TABBAR_HEIGHT;
        let tab_y = tab_bottom - 1.0;
        let first_tab_left = TAB_HORIZONTAL_PADDING;
        let first_tab_right = first_tab_left + widths[0];
        let second_tab_left = first_tab_right + TAB_ITEM_GAP;

        assert!(TerminalView::unified_titlebar_tab_shell_hit_test(
            TAB_HORIZONTAL_PADDING + 20.0,
            tab_y,
            widths,
            scroll_offset_x
        ));
        assert!(!TerminalView::unified_titlebar_tab_shell_hit_test(
            TAB_HORIZONTAL_PADDING + 240.0,
            tab_y,
            widths,
            scroll_offset_x
        ));
        assert!(TerminalView::unified_titlebar_tab_shell_hit_test(
            TAB_HORIZONTAL_PADDING + 20.0,
            TOP_STRIP_CONTENT_OFFSET_Y,
            widths,
            scroll_offset_x
        ));
        assert!(TerminalView::unified_titlebar_tab_shell_hit_test(
            TAB_HORIZONTAL_PADDING + 20.0,
            tab_top,
            widths,
            scroll_offset_x
        ));
        assert!(TerminalView::unified_titlebar_tab_shell_hit_test(
            TAB_HORIZONTAL_PADDING + 20.0,
            TOP_STRIP_CONTENT_OFFSET_Y + 1.0,
            widths,
            scroll_offset_x
        ));
        assert!(!TerminalView::unified_titlebar_tab_shell_hit_test(
            TAB_HORIZONTAL_PADDING + 20.0,
            tab_bottom,
            widths,
            scroll_offset_x
        ));
        assert!(!TerminalView::unified_titlebar_tab_shell_hit_test(
            first_tab_right,
            tab_y,
            [widths[0]],
            scroll_offset_x
        ));
        assert!(TerminalView::unified_titlebar_tab_shell_hit_test(
            second_tab_left,
            tab_y,
            widths,
            scroll_offset_x
        ));
    }

    #[test]
    fn shell_hit_test_uses_adjacent_tabs_without_gap() {
        let widths = [100.0, 120.0];
        let leading = 8.0;
        let y = tab_hit_test_y();
        // Gapless tabs: the first pixel after tab 0 already belongs to tab 1.
        assert!(
            TerminalView::unified_titlebar_tab_shell_hit_test_with_spacing(
                leading + 100.0,
                y,
                widths,
                0.0,
                leading,
                0.0,
            )
        );
        assert!(
            !TerminalView::unified_titlebar_tab_shell_hit_test_with_spacing(
                leading - 1.0,
                y,
                widths,
                0.0,
                leading,
                0.0,
            )
        );
        assert!(
            !TerminalView::unified_titlebar_tab_shell_hit_test_with_spacing(
                leading + 220.0,
                y,
                widths,
                0.0,
                leading,
                0.0,
            )
        );
    }

    #[test]
    fn interactive_hit_test_detects_tab_shell() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        let x = geometry.row_start_x + TAB_HORIZONTAL_PADDING + 12.0;
        assert!(
            TerminalView::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
                x,
                tab_hit_test_y(),
                true,
                geometry,
                [120.0, 120.0],
                0.0,
            )
        );
    }

    #[test]
    fn interactive_hit_test_detects_new_tab_button() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        let center_x = (geometry.button_start_x + geometry.button_end_x) * 0.5;
        let center_y = (geometry.button_start_y + geometry.button_end_y) * 0.5;
        assert!(
            TerminalView::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
                center_x,
                center_y,
                true,
                geometry,
                [120.0, 120.0],
                0.0,
            )
        );
    }

    #[test]
    fn interactive_hit_test_treats_action_rail_as_new_tab_button() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        let x = geometry.action_rail_start_x + 1.0;
        let y = (geometry.button_start_y + geometry.button_end_y) * 0.5;
        assert!(geometry.new_tab_button_contains(x, y));
        assert!(geometry.contains_action_rail_x(x));
        assert!(
            TerminalView::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
                x,
                y,
                true,
                geometry,
                [120.0, 120.0],
                0.0,
            )
        );
    }

    #[test]
    fn interactive_hit_test_has_no_gutter_gap_between_tabs_and_new_tab_button() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        assert_eq!(geometry.gutter_width, 0.0);
        assert_eq!(geometry.tabs_viewport_end_x(), geometry.action_rail_start_x);
    }

    #[test]
    fn interactive_hit_test_excludes_expanded_left_inset_branding_space() {
        let base_left_inset = TerminalView::titlebar_left_padding_for_platform();
        let geometry = TerminalView::tab_strip_geometry_for_viewport_with_left_inset(
            1280.0,
            base_left_inset + 64.0,
        );
        let x = geometry.left_inset_width - 1.0;
        assert!(
            !TerminalView::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
                x,
                tab_hit_test_y(),
                true,
                geometry,
                [120.0, 120.0],
                0.0,
            )
        );
    }

    #[test]
    fn interactive_hit_test_excludes_right_inset_space() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        let x = geometry.action_rail_end_x() + (geometry.right_inset_width * 0.5);
        assert!(
            !TerminalView::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
                x,
                tab_hit_test_y(),
                true,
                geometry,
                [120.0, 120.0],
                0.0,
            )
        );
    }

    #[test]
    fn interactive_hit_test_respects_half_open_region_boundaries() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        let tabs_boundary = geometry.tabs_viewport_end_x();
        assert!(!geometry.contains_tabs_viewport_x(tabs_boundary));

        let action_start = geometry.gutter_end_x();
        assert!(geometry.contains_action_rail_x(action_start));
        assert!(
            !TerminalView::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
                action_start,
                tab_hit_test_y(),
                true,
                geometry,
                [120.0, 120.0],
                0.0,
            )
        );

        let action_end = geometry.action_rail_end_x();
        assert!(!geometry.contains_action_rail_x(action_end));
        assert!(
            !TerminalView::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
                action_end,
                tab_hit_test_y(),
                true,
                geometry,
                [120.0, 120.0],
                0.0,
            )
        );
    }

    #[test]
    fn hidden_horizontal_titlebar_ignores_invisible_tab_shells() {
        let geometry = TerminalView::tab_strip_geometry_for_viewport_width(1280.0);
        let x = geometry.row_start_x + TAB_HORIZONTAL_PADDING + 12.0;

        assert!(
            !TerminalView::unified_titlebar_top_chrome_interactive_hit_test_for_geometry(
                x,
                tab_hit_test_y(),
                false,
                geometry,
                [120.0],
                0.0,
            )
        );
    }
}
