use super::*;
use crate::terminal_view::tab_strip::state::TabStripOrientation;

impl TerminalView {
    pub(in super::super) fn tab_strip_orientation(&self) -> TabStripOrientation {
        match self.tab_bar_position {
            TabBarPosition::Right => TabStripOrientation::Vertical,
            TabBarPosition::Top => TabStripOrientation::Horizontal,
        }
    }

    /// Width reserved on the right for the vertical tab sidebar. Zero unless the
    /// sidebar is active and the tab-strip chrome is visible; the collapsed rail
    /// width otherwise. Feeds the terminal grid sizer and content bounds so the
    /// terminal shrinks to the left of the sidebar.
    pub(in super::super) fn effective_sidebar_width(&self) -> f32 {
        if self.tab_bar_position != TabBarPosition::Right || !self.should_render_tab_strip_chrome()
        {
            return 0.0;
        }
        if self.sidebar_collapsed {
            SIDEBAR_COLLAPSED_WIDTH
        } else {
            SIDEBAR_WIDTH
        }
    }

    pub(in super::super) fn terminal_content_position(
        &self,
        position: gpui_kit::Point<Pixels>,
    ) -> (f32, f32) {
        let x: f32 = position.x.into();
        let y: f32 = position.y.into();
        (
            // The left workspace sidebar shifts the terminal surface right,
            // so window x maps to surface x minus that inset.
            x - self.workspace_sidebar_width(),
            Self::window_y_to_terminal_content_y(y, self.terminal_content_top_inset()),
        )
    }

    pub(in super::super) fn window_y_to_terminal_content_y(
        window_y: f32,
        chrome_height: f32,
    ) -> f32 {
        window_y - chrome_height
    }

    pub(in super::super) const fn titlebar_height() -> f32 {
        if TITLEBAR_HEIGHT > TABBAR_HEIGHT {
            TITLEBAR_HEIGHT
        } else {
            TABBAR_HEIGHT
        }
    }

    pub(in super::super) fn window_titlebar_height_for(
        _sidebar_tabs: bool,
        show_tab_strip_chrome: bool,
    ) -> f32 {
        Self::window_titlebar_height_for_platform(
            cfg!(any(target_os = "macos", target_os = "windows")),
            show_tab_strip_chrome,
        )
    }

    fn window_titlebar_height_for_platform(
        preserve_native_titlebar: bool,
        show_tab_strip_chrome: bool,
    ) -> f32 {
        if preserve_native_titlebar || show_tab_strip_chrome {
            Self::titlebar_height()
        } else {
            0.0
        }
    }

    /// Height of Termy's own titlebar row. Native tab windows use AppKit's
    /// titlebar, which sits outside the content view.
    pub(in super::super) fn window_titlebar_height(&self) -> f32 {
        if self.native_window_tabs {
            0.0
        } else {
            Self::window_titlebar_height_for(false, self.should_render_tab_strip_chrome())
        }
    }

    fn terminal_content_top_inset_for(
        sidebar_tabs: bool,
        show_tab_strip_chrome: bool,
        _show_update_banner: bool,
    ) -> f32 {
        Self::window_titlebar_height_for(sidebar_tabs, show_tab_strip_chrome)
    }

    pub(in super::super) fn terminal_content_top_inset(&self) -> f32 {
        if self.native_window_tabs {
            return 0.0;
        }
        Self::terminal_content_top_inset_for(
            false,
            self.should_render_tab_strip_chrome(),
            self.update_banner_visible(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_y_to_terminal_content_y_subtracts_non_zero_chrome() {
        assert_eq!(
            TerminalView::window_y_to_terminal_content_y(120.0, 34.0),
            86.0
        );
    }

    #[test]
    fn window_y_to_terminal_content_y_is_identity_when_chrome_is_zero() {
        assert_eq!(
            TerminalView::window_y_to_terminal_content_y(120.0, 0.0),
            120.0
        );
    }

    #[test]
    fn window_y_to_terminal_content_y_can_be_negative_when_cursor_is_above_chrome() {
        assert_eq!(
            TerminalView::window_y_to_terminal_content_y(20.0, 40.0),
            -20.0
        );
    }

    #[test]
    fn window_titlebar_height_keeps_horizontal_strip_height() {
        assert_eq!(
            TerminalView::window_titlebar_height_for(false, true),
            TerminalView::titlebar_height()
        );
    }

    #[test]
    fn linux_titlebar_height_collapses_when_tab_strip_hidden() {
        assert_eq!(
            TerminalView::window_titlebar_height_for_platform(false, false),
            0.0
        );
    }

    #[test]
    fn macos_titlebar_height_is_preserved_when_tab_strip_hidden() {
        assert_eq!(
            TerminalView::window_titlebar_height_for_platform(true, false),
            TerminalView::titlebar_height()
        );
    }

    #[test]
    fn terminal_content_top_inset_ignores_floating_update_panel_for_horizontal_tabs() {
        assert_eq!(
            TerminalView::terminal_content_top_inset_for(false, true, true),
            TerminalView::titlebar_height()
        );
    }
}
