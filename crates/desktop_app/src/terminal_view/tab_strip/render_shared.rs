use super::super::*;
use super::layout::TabStripGeometry;
use super::state::{TabStripOrientation, TabStripOverflowState};

pub(super) struct TabStripRenderState {
    pub(super) geometry: TabStripGeometry,
    pub(super) content_width: f32,
    pub(super) overflow_state: TabStripOverflowState,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn tab_strip_chrome_visible_follows_auto_hide_policy_by_default() {
        assert!(!TerminalView::tab_strip_chrome_visible(
            true,
            1,
            TabBarVisibility::FollowConfig
        ));
        assert!(!TerminalView::tab_strip_chrome_visible(
            true,
            0,
            TabBarVisibility::FollowConfig
        ));
        assert!(TerminalView::tab_strip_chrome_visible(
            false,
            1,
            TabBarVisibility::FollowConfig
        ));
        assert!(TerminalView::tab_strip_chrome_visible(
            true,
            2,
            TabBarVisibility::FollowConfig
        ));
    }

    #[test]
    fn tab_strip_chrome_visible_force_hidden_overrides_visible_tab_strip() {
        assert!(!TerminalView::tab_strip_chrome_visible(
            false,
            3,
            TabBarVisibility::ForceHidden
        ));
    }

    #[test]
    fn effective_tab_bar_visibility_passes_through_visibility() {
        assert_eq!(
            TerminalView::effective_tab_bar_visibility_for_state(TabBarVisibility::FollowConfig),
            TabBarVisibility::FollowConfig
        );
        assert_eq!(
            TerminalView::effective_tab_bar_visibility_for_state(TabBarVisibility::ForceVisible),
            TabBarVisibility::ForceVisible
        );
        assert_eq!(
            TerminalView::effective_tab_bar_visibility_for_state(TabBarVisibility::ForceHidden),
            TabBarVisibility::ForceHidden
        );
    }

    #[test]
    fn tab_strip_chrome_visible_force_visible_overrides_hidden_single_tab_strip() {
        assert!(TerminalView::tab_strip_chrome_visible(
            true,
            1,
            TabBarVisibility::ForceVisible
        ));
    }

    #[test]
    fn hidden_titlebar_branding_shows_when_auto_hide_hides_single_tab_chrome() {
        assert!(TerminalView::should_render_hidden_titlebar_branding(
            true,
            1,
            TabBarVisibility::FollowConfig,
            true
        ));
    }

    #[test]
    fn hidden_titlebar_branding_shows_when_auto_hide_hides_empty_tab_chrome() {
        assert!(TerminalView::should_render_hidden_titlebar_branding(
            true,
            0,
            TabBarVisibility::FollowConfig,
            true
        ));
    }

    #[test]
    fn hidden_titlebar_branding_hides_when_branding_is_disabled() {
        assert!(!TerminalView::should_render_hidden_titlebar_branding(
            true,
            1,
            TabBarVisibility::FollowConfig,
            false
        ));
    }

    #[test]
    fn hidden_titlebar_branding_hides_when_tab_strip_chrome_is_visible() {
        assert!(!TerminalView::should_render_hidden_titlebar_branding(
            false,
            1,
            TabBarVisibility::FollowConfig,
            true
        ));
    }

    #[test]
    fn hidden_titlebar_branding_shows_when_tab_strip_is_force_hidden() {
        assert!(TerminalView::should_render_hidden_titlebar_branding(
            false,
            3,
            TabBarVisibility::ForceHidden,
            true
        ));
    }
}

impl TerminalView {
    pub(crate) fn tab_strip_chrome_visible(
        auto_hide_tabbar: bool,
        tab_count: usize,
        visibility: TabBarVisibility,
    ) -> bool {
        match visibility {
            TabBarVisibility::FollowConfig => !auto_hide_tabbar || tab_count > 1,
            TabBarVisibility::ForceVisible => true,
            TabBarVisibility::ForceHidden => false,
        }
    }

    pub(crate) fn effective_tab_bar_visibility_for_state(
        visibility: TabBarVisibility,
    ) -> TabBarVisibility {
        visibility
    }

    pub(crate) fn effective_tab_bar_visibility(&self) -> TabBarVisibility {
        Self::effective_tab_bar_visibility_for_state(self.tab_bar_visibility)
    }

    pub(crate) fn should_render_hidden_titlebar_branding(
        auto_hide_tabbar: bool,
        tab_count: usize,
        visibility: TabBarVisibility,
        show_termy_in_titlebar: bool,
    ) -> bool {
        !Self::tab_strip_chrome_visible(auto_hide_tabbar, tab_count, visibility)
            && show_termy_in_titlebar
    }

    pub(crate) fn should_render_tab_strip_chrome(&self) -> bool {
        if self.native_window_tabs {
            return false;
        }
        // The workspace sidebar needs the top chrome row for its actions and
        // for the tab strip it aligns with, so it overrides auto-hide.
        self.workspace_sidebar_visible()
            || Self::tab_strip_chrome_visible(
                self.auto_hide_tabbar,
                self.session.tabs.len(),
                self.effective_tab_bar_visibility(),
            )
    }

    pub(super) fn build_tab_strip_render_state(
        &mut self,
        window: &Window,
        left_inset_width: f32,
    ) -> TabStripRenderState {
        let viewport_width: f32 = window.viewport_size().width.into();
        let provisional_layout = Self::tab_strip_layout_for_input(
            self.horizontal_tab_layout_input(viewport_width, left_inset_width, None),
        );
        let tab_strip_viewport_width = provisional_layout.geometry.tabs_viewport_width;
        let _ = self.sync_tab_display_widths_for_viewport_if_needed(tab_strip_viewport_width);
        self.scroll_active_tab_into_view(TabStripOrientation::Horizontal);
        let fixed_content_width = self.tab_strip_fixed_content_width();
        // Windows places the "+" directly after the tabs and leaves the remaining
        // titlebar space draggable before the native caption buttons.
        #[cfg(target_os = "windows")]
        let layout = Self::tab_strip_layout_for_input(self.horizontal_tab_layout_input(
            viewport_width,
            left_inset_width,
            Some(fixed_content_width),
        ));
        // Other platforms keep the "+" right-anchored. Tabs remain left-aligned
        // inside the viewport, with the empty area available for window dragging.
        #[cfg(not(target_os = "windows"))]
        let layout = provisional_layout;
        self.set_tab_strip_layout_snapshot(layout);

        let geometry = layout.geometry;
        let content_width = fixed_content_width;
        let overflow_state = self.tab_strip_overflow_state();

        TabStripRenderState {
            geometry,
            content_width,
            overflow_state,
        }
    }
}
