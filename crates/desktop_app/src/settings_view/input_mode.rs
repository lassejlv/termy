use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyInputMode {
    CaptureAction,
    SidebarSearch,
    ActiveInput,
    Idle,
}

impl SettingsWindow {
    pub(super) fn key_input_mode_from_flags(
        capturing_action: bool,
        sidebar_search_active: bool,
        active_input_present: bool,
    ) -> KeyInputMode {
        if capturing_action {
            KeyInputMode::CaptureAction
        } else if active_input_present {
            KeyInputMode::ActiveInput
        } else if sidebar_search_active {
            KeyInputMode::SidebarSearch
        } else {
            KeyInputMode::Idle
        }
    }

    pub(super) fn key_input_mode(&self) -> KeyInputMode {
        Self::key_input_mode_from_flags(
            self.capturing_action.is_some(),
            self.sidebar_search_active,
            self.active_input.is_some()
                || self.plugin_setting_input.is_some()
                || self.ssh_input.is_some(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{KeyInputMode, SettingsWindow};

    #[test]
    fn key_input_mode_prioritizes_capture_over_other_modes() {
        let mode = SettingsWindow::key_input_mode_from_flags(true, true, true);
        assert_eq!(mode, KeyInputMode::CaptureAction);
    }

    #[test]
    fn key_input_mode_prioritizes_active_input_over_sidebar_search() {
        let mode = SettingsWindow::key_input_mode_from_flags(false, true, true);
        assert_eq!(mode, KeyInputMode::ActiveInput);
    }

    #[test]
    fn key_input_mode_uses_sidebar_search_when_no_input_is_active() {
        let mode = SettingsWindow::key_input_mode_from_flags(false, true, false);
        assert_eq!(mode, KeyInputMode::SidebarSearch);
    }

    #[test]
    fn key_input_mode_is_idle_when_no_state_is_active() {
        let mode = SettingsWindow::key_input_mode_from_flags(false, false, false);
        assert_eq!(mode, KeyInputMode::Idle);
    }
}
