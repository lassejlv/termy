use super::super::*;

impl TerminalView {
    /// Mirrors the active tab's title into the OS window title on Windows,
    /// so the taskbar and Alt+Tab tell Termy windows apart. Cheap enough to
    /// call on every frame; the window is only retitled when the text changes.
    pub(crate) fn sync_native_window_title(&mut self, window: &mut Window) {
        #[cfg(target_os = "windows")]
        {
            let title = Self::native_window_title_for_tab(
                self.session
                    .tabs
                    .get(self.session.active_tab)
                    .map(|tab| tab.title.as_str()),
            );
            self.native_titlebar.sync_window_title(window, &title);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = window;
    }

    #[cfg(any(target_os = "windows", test))]
    fn native_window_title_for_tab(tab_title: Option<&str>) -> String {
        match tab_title.map(str::trim).filter(|title| !title.is_empty()) {
            Some(title) => format!("{title} \u{2014} Termy"),
            None => "Termy".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_window_title_appends_app_name_to_tab_title() {
        assert_eq!(
            TerminalView::native_window_title_for_tab(Some("~/src/termy")),
            "~/src/termy \u{2014} Termy"
        );
    }

    #[test]
    fn native_window_title_falls_back_to_app_name() {
        assert_eq!(TerminalView::native_window_title_for_tab(None), "Termy");
        assert_eq!(
            TerminalView::native_window_title_for_tab(Some("  ")),
            "Termy"
        );
    }
}
