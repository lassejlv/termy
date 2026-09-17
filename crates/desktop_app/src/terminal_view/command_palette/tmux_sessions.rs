use super::state_tmux::{TmuxSessionRow, TmuxSessionStatusHint};
use super::*;
use crate::terminal_ui::{TmuxClient, TmuxLaunchTarget, TmuxSocketTarget};

impl TerminalView {
    pub(in crate::terminal_view) fn open_tmux_session_palette_with_intent(
        &mut self,
        intent: TmuxSessionIntent,
        cx: &mut Context<Self>,
    ) {
        let was_open = self.command_palette.is_open();
        self.command_palette.open(CommandPaletteMode::TmuxSessions);
        self.command_palette.set_tmux_session_intent(intent);
        let notify_event = if was_open {
            CommandPaletteNotifyEvent::InteractionOnly
        } else {
            CommandPaletteNotifyEvent::OpenCloseTransition
        };
        self.apply_command_palette_mode_setup(
            CommandPaletteMode::TmuxSessions,
            false,
            notify_event,
            cx,
        );
    }

    pub(super) fn tmux_active_session_name_for_session_palette(&self) -> Option<String> {
        if !self.runtime_uses_tmux() {
            return None;
        }
        Some(self.tmux_runtime().client.session_name().to_string())
    }

    pub(super) fn tmux_socket_targets_for_session_palette_for_launch(
        runtime_uses_tmux: bool,
        launch: Option<&TmuxLaunchTarget>,
    ) -> Vec<TmuxSocketTarget> {
        if !runtime_uses_tmux {
            // When native runtime is active we must still discover persistent managed
            // sessions on the dedicated socket, otherwise detach->reattach can miss them.
            return vec![TmuxSocketTarget::DedicatedTermy, TmuxSocketTarget::Default];
        }

        let Some(launch) = launch else {
            return vec![TmuxSocketTarget::Default];
        };

        match launch {
            TmuxLaunchTarget::Managed { .. } => vec![TmuxSocketTarget::DedicatedTermy],
            TmuxLaunchTarget::Session { socket, .. } => vec![socket.clone()],
        }
    }

    fn tmux_socket_targets_for_session_palette(&self) -> Vec<TmuxSocketTarget> {
        let launch = self.runtime.as_tmux().map(|runtime| &runtime.config.launch);
        Self::tmux_socket_targets_for_session_palette_for_launch(self.runtime_uses_tmux(), launch)
    }

    pub(super) fn tmux_primary_socket_target_for_session_palette(&self) -> TmuxSocketTarget {
        self.tmux_socket_targets_for_session_palette()
            .into_iter()
            .next()
            .unwrap_or(TmuxSocketTarget::Default)
    }

    fn tmux_session_list_error_text_is_ignorable(error: &str) -> bool {
        let normalized = error.to_ascii_lowercase();
        // tmux reports "socket has no server" with different stderr strings across
        // platforms/versions ("no server running on …", "error connecting to … (No such file or
        // directory)", "error connecting to … (Connection refused)", "failed to connect to server").
        // Treat these as expected empty socket states.
        normalized.contains("no server running on")
            || normalized.contains("failed to connect to server")
            || (normalized.contains("error connecting to")
                && (normalized.contains("no such file or directory")
                    || normalized.contains("connection refused")))
    }

    fn tmux_session_list_error_is_ignorable(error: &anyhow::Error) -> bool {
        error
            .chain()
            .any(|cause| Self::tmux_session_list_error_text_is_ignorable(&cause.to_string()))
    }

    fn tmux_socket_target_display_name(socket_target: &TmuxSocketTarget) -> String {
        match socket_target {
            TmuxSocketTarget::DedicatedTermy => "termy".to_string(),
            TmuxSocketTarget::Default => "default".to_string(),
            TmuxSocketTarget::Named(name) => name.clone(),
        }
    }

    /// Resolve the (command prefix, binary) pair used to spawn out-of-band
    /// tmux commands for the session palette.
    fn tmux_invocation_for_session_palette(&mut self) -> Result<(Vec<String>, String), String> {
        let (command_prefix, binary) = if self.runtime_uses_tmux() {
            (
                self.tmux_runtime().config.command_prefix.clone(),
                self.tmux_runtime().config.binary.trim().to_string(),
            )
        } else if let Some(cached) = self
            .cached_tmux_binary
            .as_deref()
            .map(str::trim)
            .filter(|cached| !cached.is_empty())
        {
            (self.cached_tmux_command_prefix.clone(), cached.to_string())
        } else {
            let loaded = config::load_runtime_config(
                &mut self.last_config_error_message,
                "Failed to read config for tmux session listing",
            );
            let loaded_binary = loaded.config.tmux_binary.trim().to_string();
            self.cached_tmux_binary = (!loaded_binary.is_empty()).then_some(loaded_binary.clone());
            let loaded_prefix = loaded.config.tmux_command_prefix_argv();
            self.cached_tmux_command_prefix = loaded_prefix.clone();
            (loaded_prefix, loaded_binary)
        };
        if binary.is_empty() {
            return Err("tmux_binary must not be empty".to_string());
        }
        Ok((command_prefix, binary))
    }

    pub(super) fn reload_tmux_session_palette_items(&mut self) -> Result<(), String> {
        let socket_targets = self.tmux_socket_targets_for_session_palette();
        let create_socket_target = socket_targets
            .first()
            .cloned()
            .unwrap_or(TmuxSocketTarget::Default);
        let (command_prefix, binary) = self.tmux_invocation_for_session_palette()?;
        let mut rows = Vec::<TmuxSessionRow>::new();
        let mut failures = Vec::<String>::new();

        for socket_target in socket_targets {
            match TmuxClient::list_sessions(&command_prefix, binary.as_str(), socket_target.clone())
            {
                Ok(sessions) => {
                    rows.extend(sessions.into_iter().map(|summary| TmuxSessionRow {
                        summary,
                        socket_target: socket_target.clone(),
                    }));
                }
                Err(error) => {
                    if !Self::tmux_session_list_error_is_ignorable(&error) {
                        let error_text = format!("{error:#}");
                        failures.push(format!(
                            "{} socket: {error_text}",
                            Self::tmux_socket_target_display_name(&socket_target)
                        ));
                    }
                }
            }
        }

        self.command_palette
            .set_tmux_session_rows(rows, create_socket_target);

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join(" | "))
        }
    }

    pub(super) fn command_palette_disabled_tmux_session_message(
        status_hint: Option<TmuxSessionStatusHint>,
    ) -> &'static str {
        match status_hint {
            Some(TmuxSessionStatusHint::ActiveSession) => {
                "Detach or switch tmux session before renaming or killing the active session"
            }
            Some(TmuxSessionStatusHint::NameRequired) => "tmux session name cannot be empty",
            Some(TmuxSessionStatusHint::NameUnchanged) => {
                "New tmux session name must differ from current name"
            }
            _ => "tmux session action is unavailable",
        }
    }

    pub(super) fn activate_tmux_session_from_palette(
        &mut self,
        session_name: &str,
        socket_target: TmuxSocketTarget,
        enabled: bool,
        status_hint: Option<TmuxSessionStatusHint>,
        cx: &mut Context<Self>,
    ) {
        if !enabled {
            crate::ui::toast::info(Self::command_palette_disabled_tmux_session_message(
                status_hint,
            ));
            self.notify_overlay(cx);
            return;
        }

        let session_name = session_name.trim();
        if session_name.is_empty() {
            crate::ui::toast::error("tmux session name cannot be empty");
            self.notify_overlay(cx);
            return;
        }

        let launch = TmuxLaunchTarget::Session {
            name: session_name.to_string(),
            socket: socket_target,
        };
        if self.attach_tmux_runtime(launch, cx) {
            self.close_command_palette(cx);
            self.notify_overlay(cx);
        }
    }

    pub(super) fn detach_current_tmux_session_from_palette(&mut self, cx: &mut Context<Self>) {
        if self.detach_tmux_runtime_to_native(cx) {
            self.close_command_palette(cx);
            self.notify_overlay(cx);
        }
    }

    pub(super) fn open_tmux_session_rename_mode_from_palette(&mut self, cx: &mut Context<Self>) {
        self.command_palette
            .set_tmux_session_intent(TmuxSessionIntent::RenameSelect);
        self.command_palette.input_mut().clear();
        self.refresh_command_palette_matches(false, cx);
        self.notify_overlay(cx);
    }

    pub(super) fn open_tmux_session_kill_mode_from_palette(&mut self, cx: &mut Context<Self>) {
        self.command_palette
            .set_tmux_session_intent(TmuxSessionIntent::Kill);
        self.command_palette.input_mut().clear();
        self.refresh_command_palette_matches(false, cx);
        self.notify_overlay(cx);
    }

    pub(super) fn select_tmux_session_for_rename_from_palette(
        &mut self,
        session_name: &str,
        socket_target: TmuxSocketTarget,
        enabled: bool,
        status_hint: Option<TmuxSessionStatusHint>,
        cx: &mut Context<Self>,
    ) {
        if !enabled {
            crate::ui::toast::info(Self::command_palette_disabled_tmux_session_message(
                status_hint,
            ));
            self.notify_overlay(cx);
            return;
        }

        self.command_palette
            .begin_tmux_session_rename(session_name, socket_target);
        self.refresh_command_palette_matches(false, cx);
        self.notify_overlay(cx);
    }

    pub(super) fn refresh_tmux_session_palette_after_lifecycle_action(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self.reload_tmux_session_palette_items() {
            self.command_palette.set_tmux_session_rows(
                Vec::new(),
                self.tmux_primary_socket_target_for_session_palette(),
            );
            crate::ui::toast::error(format!("Failed to list tmux sessions: {error}"));
        }
        self.refresh_command_palette_matches(false, cx);
        self.notify_overlay(cx);
    }

    pub(super) fn apply_tmux_session_rename_from_palette(
        &mut self,
        current_session_name: &str,
        next_session_name: &str,
        socket_target: TmuxSocketTarget,
        enabled: bool,
        status_hint: Option<TmuxSessionStatusHint>,
        cx: &mut Context<Self>,
    ) {
        if !enabled {
            crate::ui::toast::info(Self::command_palette_disabled_tmux_session_message(
                status_hint,
            ));
            self.notify_overlay(cx);
            return;
        }

        let (command_prefix, binary) = match self.tmux_invocation_for_session_palette() {
            Ok(invocation) => invocation,
            Err(error) => {
                crate::ui::toast::error(error);
                self.notify_overlay(cx);
                return;
            }
        };

        if let Err(error) = TmuxClient::rename_session(
            &command_prefix,
            binary.as_str(),
            socket_target,
            current_session_name,
            next_session_name,
        ) {
            crate::ui::toast::error(format!("Failed to rename tmux session: {error}"));
            self.notify_overlay(cx);
            return;
        }

        self.command_palette
            .set_tmux_session_intent(TmuxSessionIntent::RenameSelect);
        self.command_palette.input_mut().clear();
        self.refresh_tmux_session_palette_after_lifecycle_action(cx);
    }

    pub(super) fn confirm_kill_tmux_session_from_palette(
        &mut self,
        session_name: &str,
        socket_target: TmuxSocketTarget,
        enabled: bool,
        status_hint: Option<TmuxSessionStatusHint>,
        cx: &mut Context<Self>,
    ) {
        if !enabled {
            crate::ui::toast::info(Self::command_palette_disabled_tmux_session_message(
                status_hint,
            ));
            self.notify_overlay(cx);
            return;
        }

        let session_name = session_name.trim().to_string();
        if session_name.is_empty() {
            crate::ui::toast::error("tmux session name cannot be empty");
            self.notify_overlay(cx);
            return;
        }

        let confirmation_message = format!(
            "Kill tmux session \"{session_name}\"? This will close all windows and panes in that session."
        );
        // Native confirm dialogs can run nested modal loops; invoking them while GPUI
        // is mutably updating this view can re-enter and trip RefCell borrow checks.
        // Run confirm out-of-band, then re-enter through AsyncApp for the mutation.
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let confirmed = crate::native_sdk::confirm("Kill tmux Session", &confirmation_message);
            if !confirmed {
                return;
            }

            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    let (command_prefix, binary) = match view.tmux_invocation_for_session_palette()
                    {
                        Ok(invocation) => invocation,
                        Err(error) => {
                            crate::ui::toast::error(error);
                            view.notify_overlay(cx);
                            return;
                        }
                    };

                    if let Err(error) = TmuxClient::kill_session(
                        &command_prefix,
                        binary.as_str(),
                        socket_target,
                        session_name.as_str(),
                    ) {
                        crate::ui::toast::error(format!("Failed to kill tmux session: {error}"));
                        view.notify_overlay(cx);
                        return;
                    }

                    view.refresh_tmux_session_palette_after_lifecycle_action(cx);
                })
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn native_session_palette_prioritizes_managed_socket_discovery_order() {
        assert_eq!(
            TerminalView::tmux_socket_targets_for_session_palette_for_launch(false, None),
            vec![TmuxSocketTarget::DedicatedTermy, TmuxSocketTarget::Default]
        );
    }

    #[test]
    fn tmux_runtime_session_palette_uses_active_runtime_socket() {
        assert_eq!(
            TerminalView::tmux_socket_targets_for_session_palette_for_launch(
                true,
                Some(&TmuxLaunchTarget::Managed { persistence: true }),
            ),
            vec![TmuxSocketTarget::DedicatedTermy]
        );
        assert_eq!(
            TerminalView::tmux_socket_targets_for_session_palette_for_launch(
                true,
                Some(&TmuxLaunchTarget::Session {
                    name: "work".to_string(),
                    socket: TmuxSocketTarget::Named("work".to_string()),
                }),
            ),
            vec![TmuxSocketTarget::Named("work".to_string())]
        );
    }

    #[test]
    fn tmux_session_list_error_classifier_ignores_expected_no_server_variants() {
        assert!(TerminalView::tmux_session_list_error_text_is_ignorable(
            "no server running on /private/tmp/tmux-501/default"
        ));
        assert!(TerminalView::tmux_session_list_error_text_is_ignorable(
            "error connecting to /tmp/tmux-1000/default (No such file or directory)"
        ));
        assert!(TerminalView::tmux_session_list_error_text_is_ignorable(
            "failed to connect to server"
        ));
        assert!(TerminalView::tmux_session_list_error_text_is_ignorable(
            "error connecting to /tmp/tmux-1000/default (Connection refused)"
        ));
    }

    #[test]
    fn tmux_session_list_error_classifier_preserves_real_failures() {
        assert!(!TerminalView::tmux_session_list_error_text_is_ignorable(
            "tmux session listing failed: list-sessions -F '#{q:session_name}'"
        ));
    }

    #[test]
    fn tmux_session_list_error_classifier_uses_full_error_chain() {
        let wrapped =
            anyhow!("error connecting to /tmp/tmux-1000/default (No such file or directory)")
                .context("tmux session listing failed: list-sessions -F '#{q:session_name}'");
        assert!(TerminalView::tmux_session_list_error_is_ignorable(&wrapped));

        let wrapped_real_failure = anyhow!("unsupported format string")
            .context("tmux session listing failed: list-sessions -F '#{q:session_name}'");
        assert!(!TerminalView::tmux_session_list_error_is_ignorable(
            &wrapped_real_failure
        ));
    }
}
