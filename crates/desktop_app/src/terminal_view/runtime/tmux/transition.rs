use super::*;
use termy_terminal_ui::{TmuxClient, TmuxLaunchTarget, TmuxRuntimeConfig};

impl TerminalView {
    fn refresh_runtime_capability_surfaces(&mut self, cx: &mut Context<Self>) {
        let loaded = config::load_runtime_config(
            &mut self.last_config_error_message,
            "Failed to reload config after tmux runtime transition",
        );
        let mut simple_mode = self.simple_mode;
        if loaded.loaded_from_disk {
            let keybind_config = loaded.config;
            simple_mode = keybind_config.simple_mode;
            self.simple_mode = keybind_config.simple_mode;
            let binary = keybind_config.tmux_binary.trim().to_string();
            self.cached_tmux_binary = (!binary.is_empty()).then_some(binary);
            self.cached_tmux_command_prefix = keybind_config.tmux_command_prefix_argv();
            keybindings::install_keybindings(cx, &keybind_config, self.runtime_uses_tmux());
        }
        cx.set_menus(crate::menus::app_menus(
            self.install_cli_available(),
            self.runtime_uses_tmux(),
            simple_mode,
        ));
        self.refresh_command_palette_items_for_current_mode(cx);
    }

    fn create_native_runtime_tab_for_size(
        &self,
        size: TerminalSize,
    ) -> anyhow::Result<TerminalTab> {
        let terminal = Terminal::new_native(
            size,
            self.configured_working_dir.as_deref(),
            Some(&self.native_terminal_wakeup_router),
            Some(&self.tab_shell_integration),
            Some(&self.terminal_runtime),
            None,
        )?;
        let predicted_prompt_cwd = Self::predicted_prompt_cwd(
            self.configured_working_dir.as_deref(),
            self.terminal_runtime.working_dir_fallback,
        );
        let predicted_title =
            Self::predicted_prompt_seed_title(&self.tab_title, predicted_prompt_cwd.as_deref());
        let tab_id = self.next_tab_id;
        Ok(Self::create_native_tab(
            tab_id,
            terminal,
            size.cols,
            size.rows,
            predicted_title,
        ))
    }

    pub(in crate::terminal_view) fn attach_tmux_runtime(
        &mut self,
        launch: TmuxLaunchTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        let (binary, command_prefix, show_active_pane_border) = if self.runtime_uses_tmux() {
            (
                self.tmux_runtime().config.binary.clone(),
                self.tmux_runtime().config.command_prefix.clone(),
                self.tmux_runtime().config.show_active_pane_border,
            )
        } else {
            let loaded = config::load_runtime_config(
                &mut self.last_config_error_message,
                "Failed to read config for tmux attach",
            );
            if loaded.loaded_from_disk {
                let loaded_binary = loaded.config.tmux_binary.trim().to_string();
                if !loaded_binary.is_empty() {
                    self.cached_tmux_binary = Some(loaded_binary.clone());
                }
                let loaded_prefix = loaded.config.tmux_command_prefix_argv();
                self.cached_tmux_command_prefix = loaded_prefix.clone();
                (
                    loaded_binary,
                    loaded_prefix,
                    loaded.config.tmux_show_active_pane_border,
                )
            } else {
                (
                    self.cached_tmux_binary.clone().unwrap_or_default(),
                    self.cached_tmux_command_prefix.clone(),
                    self.tmux_show_active_pane_border,
                )
            }
        };
        let runtime_config = TmuxRuntimeConfig {
            binary,
            command_prefix,
            launch,
            show_active_pane_border,
        };
        if let Err(error) = TmuxClient::verify_tmux_version(
            &runtime_config.command_prefix,
            runtime_config.binary.as_str(),
            3,
            3,
        ) {
            termy_toast::error(format!("tmux preflight failed: {error}"));
            return false;
        }

        // Runtime transitions can briefly observe an empty tab/pane set; fall back
        // to the default grid size rather than panicking on a missing active pane.
        let size = self
            .active_terminal()
            .map(|terminal| terminal.size())
            .unwrap_or_default();
        let initial_working_dir = self.preferred_working_dir_for_new_session(None, cx);
        let tmux_client = match TmuxClient::new(
            runtime_config.clone(),
            size.cols.max(1),
            size.rows.max(1),
            initial_working_dir.as_deref(),
            Some(self.event_wakeup_tx.clone()),
        ) {
            Ok(client) => client,
            Err(error) => {
                termy_toast::error(format!("failed to start tmux control runtime: {error}"));
                return false;
            }
        };
        let snapshot = match tmux_client.refresh_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if let Err(cleanup_error) = tmux_client.shutdown_default() {
                    termy_toast::error(format!(
                        "failed to fetch tmux snapshot: {error}; cleanup failed: {cleanup_error}"
                    ));
                    return false;
                }
                termy_toast::error(format!("failed to fetch tmux snapshot: {error}"));
                return false;
            }
        };

        if self.runtime_uses_tmux()
            && let Err(error) = self.tmux_runtime().client.shutdown_default()
        {
            // Switching sessions must be a hard cutover. If the previous client
            // cannot be detached, abort and explicitly cleanup the freshly spawned
            // client to avoid accumulating orphaned control clients.
            let new_cleanup_result = tmux_client.shutdown_default();
            match tmux_cutover_cleanup_decision(false, new_cleanup_result.is_ok()) {
                TmuxCutoverCleanupDecision::AbortOldAndNewCleanupFailure => {
                    let cleanup_error = new_cleanup_result.expect_err(
                        "new cleanup error must be present when decision is combined failure",
                    );
                    termy_toast::error(format!(
                        "failed to cleanup previous tmux client before attach: {error}; \
                         failed to cleanup new tmux client: {cleanup_error}"
                    ));
                    return false;
                }
                TmuxCutoverCleanupDecision::AbortOldCleanupFailure => {
                    termy_toast::error(format!(
                        "failed to cleanup previous tmux client before attach: {error}"
                    ));
                    return false;
                }
                TmuxCutoverCleanupDecision::Proceed => {}
            }
        }

        self.runtime = RuntimeState::Tmux(TmuxRuntime::new(
            runtime_config,
            tmux_client,
            initial_working_dir,
            size.cols.max(1),
            size.rows.max(1),
        ));
        self.native_pane_layout_trees.clear();
        self.native_pane_zoom_snapshots.clear();
        self.apply_tmux_snapshot_rehydrate(snapshot);
        self.reset_tab_interaction_state();
        self.clear_selection();
        self.refresh_runtime_capability_surfaces(cx);
        cx.notify();
        true
    }

    fn commit_tmux_runtime_to_native(&mut self, native_tab: TerminalTab, cx: &mut Context<Self>) {
        self.runtime = RuntimeState::Native;
        self.tabs = vec![native_tab];
        self.native_pane_layout_trees.clear();
        self.native_pane_zoom_snapshots.clear();
        self.active_tab = 0;
        self.next_tab_id = self.tabs[0].id.saturating_add(1);
        self.refresh_tab_title(0);
        self.mark_tab_strip_layout_dirty();
        self.reset_tab_interaction_state();
        self.clear_selection();
        self.scroll_active_tab_into_view(self.tab_strip_orientation());
        self.refresh_runtime_capability_surfaces(cx);
        cx.notify();
    }

    fn transition_tmux_runtime_to_native(
        &mut self,
        size: TerminalSize,
        cx: &mut Context<Self>,
    ) -> bool {
        let native_tab = match self.create_native_runtime_tab_for_size(size) {
            Ok(tab) => tab,
            Err(error) => {
                termy_toast::error(format!("Failed to start native runtime: {error}"));
                return false;
            }
        };

        self.commit_tmux_runtime_to_native(native_tab, cx);
        true
    }

    pub(in crate::terminal_view) fn detach_tmux_runtime_to_native(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.runtime_uses_tmux() {
            return false;
        }
        if self.tmux_exclusive {
            termy_toast::info(
                "tmux_exclusive keeps Termy in control mode; disable it to detach to a classic terminal",
            );
            return false;
        }

        // Detach should remain recoverable even if tmux just invalidated panes.
        let size = self
            .active_terminal()
            .map(|terminal| terminal.size())
            .unwrap_or_default();
        let native_tab = match self.create_native_runtime_tab_for_size(size) {
            Ok(tab) => tab,
            Err(error) => {
                termy_toast::error(format!("Failed to start native runtime: {error}"));
                return false;
            }
        };

        let shutdown_result = self.tmux_runtime().client.shutdown_default();
        match tmux_detach_transition_decision(true, shutdown_result.is_ok()) {
            TmuxDetachTransitionDecision::CommitNativeTransition => {
                self.commit_tmux_runtime_to_native(native_tab, cx);
                termy_toast::success("Detached tmux session");
                true
            }
            TmuxDetachTransitionDecision::AbortTmuxShutdown => {
                let error = shutdown_result.expect_err(
                    "shutdown error must be present when decision aborts tmux shutdown",
                );
                termy_toast::error(format!("Failed to detach tmux session: {error}"));
                false
            }
            TmuxDetachTransitionDecision::AbortNativeRuntimeStart => {
                unreachable!("native runtime is already initialized at this stage")
            }
        }
    }

    pub(in crate::terminal_view) fn recover_from_tmux_runtime_exit(
        &mut self,
        reason: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.runtime_uses_tmux() {
            return false;
        }

        match tmux_exit_recovery_decision(self.tmux_exclusive) {
            TmuxExitRecoveryDecision::RestartControlMode => {
                if let Some(reason) = reason {
                    termy_toast::warning(format!(
                        "{reason}; restarting tmux control mode (tmux_exclusive)"
                    ));
                } else {
                    termy_toast::warning("tmux control mode exited; restarting (tmux_exclusive)");
                }
                self.restart_tmux_runtime_after_exit(cx)
            }
            TmuxExitRecoveryDecision::TransitionToNative => {
                // Exit recovery uses a deterministic fallback when no active pane remains.
                let size = self
                    .active_terminal()
                    .map(|terminal| terminal.size())
                    .unwrap_or_default();
                if let Some(reason) = reason {
                    termy_toast::error(reason);
                }
                self.transition_tmux_runtime_to_native(size, cx)
            }
        }
    }

    /// Force-restart control mode after the previous client already exited.
    /// Best-effort cleanup of the dead client must not block spawning a replacement.
    fn restart_tmux_runtime_after_exit(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.runtime_uses_tmux() {
            return false;
        }

        let runtime_config = self.tmux_runtime().config.clone();
        let cols = self.tmux_runtime().client_cols.max(1);
        let rows = self.tmux_runtime().client_rows.max(1);
        let initial_working_dir = self.tmux_runtime().preferred_cwd.clone().or_else(|| {
            termy_terminal_ui::resolve_launch_working_directory(
                self.configured_working_dir.as_deref(),
                self.terminal_runtime.working_dir_fallback,
            )
            .map(|path| path.to_string_lossy().into_owned())
        });

        if let Err(error) = TmuxClient::verify_tmux_version(
            &runtime_config.command_prefix,
            runtime_config.binary.as_str(),
            3,
            3,
        ) {
            termy_toast::error(format!("tmux exclusive restart failed: {error}"));
            return false;
        }

        let next_client = match TmuxClient::new(
            runtime_config.clone(),
            cols,
            rows,
            initial_working_dir.as_deref(),
            Some(self.event_wakeup_tx.clone()),
        ) {
            Ok(client) => client,
            Err(error) => {
                termy_toast::error(format!(
                    "tmux exclusive restart failed to start control mode: {error}"
                ));
                return false;
            }
        };

        let snapshot = match next_client.refresh_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = next_client.shutdown_default();
                termy_toast::error(format!(
                    "tmux exclusive restart failed to fetch snapshot: {error}"
                ));
                return false;
            }
        };

        // Previous control client already exited; ignore cleanup failures.
        let _ = self.tmux_runtime().client.shutdown_default();

        self.runtime = RuntimeState::Tmux(TmuxRuntime::new(
            runtime_config,
            next_client,
            initial_working_dir,
            cols,
            rows,
        ));
        self.native_pane_layout_trees.clear();
        self.native_pane_zoom_snapshots.clear();
        self.apply_tmux_snapshot_rehydrate(snapshot);
        self.reset_tab_interaction_state();
        self.clear_selection();
        self.refresh_runtime_capability_surfaces(cx);
        cx.notify();
        true
    }

    pub(in crate::terminal_view) fn tmux_client_cols(&self) -> u16 {
        self.tmux_runtime().client_cols
    }

    pub(in crate::terminal_view) fn tmux_client_rows(&self) -> u16 {
        self.tmux_runtime().client_rows
    }

    pub(in crate::terminal_view) fn sync_tmux_client_size(
        &mut self,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<()> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        {
            let runtime = self.tmux_runtime_mut();
            runtime.client.set_client_size(cols, rows)?;
            runtime.client_cols = cols;
            runtime.client_rows = rows;
        }
        // Snapshot convergence is scheduled asynchronously so UI resize never blocks.
        self.request_tmux_resize_convergence(cols, rows);
        Ok(())
    }

    pub(in crate::terminal_view) fn reconnect_tmux_runtime(
        &mut self,
        next_config: TmuxRuntimeConfig,
    ) {
        if !self.runtime_uses_tmux() {
            return;
        }

        if self.tmux_runtime().config == next_config {
            return;
        }

        let cols = self.tmux_runtime().client_cols.max(1);
        let rows = self.tmux_runtime().client_rows.max(1);
        let initial_working_dir = self.tmux_runtime().preferred_cwd.clone().or_else(|| {
            termy_terminal_ui::resolve_launch_working_directory(
                self.configured_working_dir.as_deref(),
                self.terminal_runtime.working_dir_fallback,
            )
            .map(|path| path.to_string_lossy().into_owned())
        });
        if let Err(error) = TmuxClient::verify_tmux_version(
            &next_config.command_prefix,
            next_config.binary.as_str(),
            3,
            3,
        ) {
            termy_toast::error(format!("tmux preflight failed: {error}"));
            return;
        }
        match TmuxClient::new(
            next_config.clone(),
            cols,
            rows,
            initial_working_dir.as_deref(),
            Some(self.event_wakeup_tx.clone()),
        ) {
            Ok(next_client) => {
                if let Err(error) = self.tmux_runtime().client.shutdown_default() {
                    let new_cleanup_result = next_client.shutdown_default();
                    match tmux_cutover_cleanup_decision(false, new_cleanup_result.is_ok()) {
                        TmuxCutoverCleanupDecision::AbortOldAndNewCleanupFailure => {
                            let cleanup_error = new_cleanup_result.expect_err(
                                "new cleanup error must be present when decision is combined failure",
                            );
                            termy_toast::error(format!(
                                "tmux reconnect failed while cleaning previous client: {error}; \
                                 failed to cleanup new client: {cleanup_error}"
                            ));
                            return;
                        }
                        TmuxCutoverCleanupDecision::AbortOldCleanupFailure => {
                            termy_toast::error(format!(
                                "tmux reconnect failed while cleaning previous client: {error}"
                            ));
                            return;
                        }
                        TmuxCutoverCleanupDecision::Proceed => {}
                    }
                }

                let runtime = self.tmux_runtime_mut();
                runtime.config = next_config;
                runtime.client = next_client;
                runtime.preferred_cwd = initial_working_dir;
                runtime.resize_scheduler.clear();
                runtime.resize_wakeup_scheduled = false;
                runtime.title_refresh_deadline = None;
                runtime.title_refresh_wakeup_scheduled = false;
                let _ = self.refresh_tmux_snapshot();
            }
            Err(error) => {
                termy_toast::error(format!("tmux reconnect failed: {error}"));
            }
        }
    }
}
