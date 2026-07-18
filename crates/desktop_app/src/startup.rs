/// Startup failures that should block app launch with actionable recovery guidance.
#[allow(clippy::enum_variant_names)]
pub(crate) enum StartupBlocker {
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    TmuxPreflight(String),
    MainWindowOpen(String),
}

impl StartupBlocker {
    fn tmux_reason_and_error(&self) -> (&'static str, &str) {
        match self {
            Self::TmuxPreflight(error) => ("tmux preflight failed", error),
            Self::MainWindowOpen(_) => unreachable!("main-window failures are not tmux failures"),
        }
    }

    pub(crate) fn tmux_fallback_message(&self) -> String {
        let (reason, error) = self.tmux_reason_and_error();
        format!("tmux is unavailable ({reason}: {error}); starting in native mode")
    }

    pub(crate) fn message(&self) -> String {
        if let Self::MainWindowOpen(error) = self {
            return format!(
                "Termy cannot continue because it failed to open the main window.\n\nError:\n{error}\n\nRecovery:\n- Restart Termy and try again.\n- If this was launched from a terminal, keep this stderr message for support.\n- If the problem repeats, include your OS, display/GPU setup, and recent Termy logs in the bug report."
            );
        }

        let (reason, error) = self.tmux_reason_and_error();

        format!(
            "Termy cannot continue because {reason}.\n\nError:\n{error}\n\nRecovery:\n- Open your config and set tmux_enabled = false to start in native mode.\n- Finder/DMG launches use a minimal environment; set tmux_binary to an absolute path (for example /opt/homebrew/bin/tmux) if tmux is not on the default PATH.\n- If tmux integration is desired, ensure tmux 3.3 or newer is installed.\n- Save the config and restart Termy, then use tmux Sessions… when ready."
        )
    }

    pub(crate) fn present_alert_and_exit(self) -> ! {
        let message = self.message();
        eprintln!("Termy startup blocked:\n{message}");
        termy_native_sdk::show_alert("Termy Startup Error", &message);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::StartupBlocker;

    #[test]
    fn startup_blocker_message_includes_tmux_guidance() {
        let message = StartupBlocker::TmuxPreflight("tmux 3.3+ required".to_string()).message();
        assert!(message.contains("tmux 3.3+ required"));
        assert!(message.contains("tmux_enabled"));
        assert!(message.contains("tmux Sessions…"));
        assert!(message.contains("Finder/DMG"));
        assert!(message.contains("/opt/homebrew/bin/tmux"));
        assert!(message.contains("restart"));
    }

    #[test]
    fn tmux_fallback_message_explains_native_recovery() {
        let message = StartupBlocker::TmuxPreflight("tmux executable was not found".to_string())
            .tmux_fallback_message();
        assert!(message.contains("tmux executable was not found"));
        assert!(message.contains("starting in native mode"));
    }

    #[test]
    fn startup_blocker_message_for_main_window_open_includes_recovery() {
        let message = StartupBlocker::MainWindowOpen("no display available".to_string()).message();
        assert!(message.contains("failed to open the main window"));
        assert!(message.contains("no display available"));
        assert!(message.contains("Restart Termy"));
        assert!(message.contains("stderr"));
        assert!(message.contains("display/GPU"));
    }
}
