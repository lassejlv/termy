use super::*;
use termy_command_core::{CommandCapabilities, CommandUnavailableReason};

impl TerminalView {
    fn shortcut_action_allowed_with_active_inline_input(action: CommandAction) -> bool {
        matches!(
            action,
            CommandAction::Copy | CommandAction::Paste | CommandAction::SelectAll
        )
    }

    fn command_palette_mode_for_action(action: CommandAction) -> Option<CommandPaletteMode> {
        match action {
            CommandAction::SwitchTheme => Some(CommandPaletteMode::Themes),
            CommandAction::ManageTmuxSessions => Some(CommandPaletteMode::TmuxSessions),
            CommandAction::ManageSavedLayouts => Some(CommandPaletteMode::Layouts),
            CommandAction::RunTask => Some(CommandPaletteMode::Tasks),
            _ => None,
        }
    }

    fn simple_mode_blocks_command_action(action: CommandAction) -> bool {
        matches!(
            action,
            CommandAction::ToggleCommandPalette
                | CommandAction::SwitchTheme
                | CommandAction::ManageTmuxSessions
                | CommandAction::ManageSavedLayouts
                | CommandAction::RunTask
        )
    }

    fn command_shortcuts_suspended(&self) -> bool {
        self.has_active_inline_input()
    }

    fn plugin_ui_blocks_terminal_action(
        plugin_ui_open: bool,
        respect_shortcut_suspend: bool,
    ) -> bool {
        plugin_ui_open && respect_shortcut_suspend
    }

    /// Single source for runtime command gating so the palette, menus, and
    /// execution path all apply the same capability rules.
    pub(in super::super) fn command_capabilities(&self) -> CommandCapabilities {
        CommandCapabilities {
            tmux_runtime_active: self.runtime_uses_tmux(),
            install_cli_available: self.install_cli_available(),
        }
    }

    fn maybe_suppress_tab_switch_hint_for_action(
        &mut self,
        action: CommandAction,
        cx: &mut Context<Self>,
    ) {
        if self.tab_strip.switch_hints.suppress_for_action(
            action,
            self.tab_switch_hints_blocked(),
            Instant::now(),
        ) {
            cx.notify();
        }
    }

    pub(in super::super) fn execute_command_action(
        &mut self,
        action: CommandAction,
        respect_shortcut_suspend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if Self::plugin_ui_blocks_terminal_action(
            self.plugin_ui.is_some(),
            respect_shortcut_suspend,
        ) {
            return;
        }
        self.maybe_suppress_tab_switch_hint_for_action(action, cx);

        #[cfg(target_os = "windows")]
        if action == CommandAction::ManageTmuxSessions || action.to_command_id().is_tmux_only() {
            // Defensive guard: custom keybinds can still target tmux actions even when
            // Windows UI entries are hidden.
            crate::ui::toast::info("tmux integration is unsupported on Windows");
            self.notify_overlay(cx);
            return;
        }

        // Keep runtime command gating aligned with command_core so every UI surface
        // and execution path applies the same capability rules.
        let availability = action.availability(self.command_capabilities());
        if !availability.enabled {
            match availability.reason {
                Some(CommandUnavailableReason::RequiresTmuxRuntime) => {
                    crate::ui::toast::info("Attach a tmux session to use this command");
                    self.notify_overlay(cx);
                    return;
                }
                Some(CommandUnavailableReason::InstallCliAlreadyInstalled) => {
                    crate::ui::toast::info("CLI is already installed");
                    self.notify_overlay(cx);
                    return;
                }
                _ => {
                    log::warn!(
                        "command reported unavailable without a known reason: action={:?} reason={:?}",
                        action,
                        availability.reason
                    );
                    crate::ui::toast::info("Command unavailable");
                    self.notify_overlay(cx);
                    return;
                }
            }
        }

        if self.simple_mode && Self::simple_mode_blocks_command_action(action) {
            if self.is_command_palette_open() {
                self.close_command_palette(cx);
            }
            return;
        }

        let shortcuts_suspended = respect_shortcut_suspend
            && self.command_shortcuts_suspended()
            && !Self::shortcut_action_allowed_with_active_inline_input(action);

        match action {
            CommandAction::ToggleCommandPalette => {
                if self.is_command_palette_open() {
                    self.close_command_palette(cx);
                } else {
                    self.open_command_palette(cx);
                }
            }
            CommandAction::SwitchTheme => {
                if let Some(mode) = Self::command_palette_mode_for_action(action) {
                    self.open_command_palette_in_mode(mode, cx);
                }
            }
            CommandAction::ManageTmuxSessions => {
                self.open_tmux_session_palette_with_intent(TmuxSessionIntent::AttachOrSwitch, cx);
            }
            CommandAction::ManageSavedLayouts => {
                self.open_saved_layouts_palette(cx);
            }
            CommandAction::RunTask => {
                self.open_tasks_palette(cx);
            }
            CommandAction::Quit => {
                self.execute_quit_command_action(action, window, cx);
            }
            CommandAction::ToggleTabBarVisibility => {
                let next_visibility = if self.should_render_tab_strip_chrome() {
                    TabBarVisibility::ForceHidden
                } else {
                    TabBarVisibility::ForceVisible
                };
                if self.set_tab_bar_visibility(next_visibility) {
                    cx.notify();
                }
            }
            CommandAction::ToggleWorkspaceSidebar => {
                self.toggle_workspace_sidebar_collapsed(cx);
            }
            CommandAction::ToggleInspector => {
                self.toggle_inspector(cx);
            }
            _ if shortcuts_suspended => {}
            CommandAction::OpenConfig
            | CommandAction::PrettifyConfig
            | CommandAction::ImportColors
            | CommandAction::AppInfo
            | CommandAction::OpenSettings
            | CommandAction::CheckForUpdates => {
                self.execute_app_system_command_action(action, cx);
            }
            CommandAction::RestartApp => {
                self.execute_quit_command_action(action, window, cx);
            }
            CommandAction::RenameTab
            | CommandAction::NewTab
            | CommandAction::CloseTab
            | CommandAction::ClosePaneOrTab
            | CommandAction::MoveTabLeft
            | CommandAction::MoveTabRight
            | CommandAction::SwitchTabLeft
            | CommandAction::SwitchTabRight
            | CommandAction::CycleTabs
            | CommandAction::SwitchToTab1
            | CommandAction::SwitchToTab2
            | CommandAction::SwitchToTab3
            | CommandAction::SwitchToTab4
            | CommandAction::SwitchToTab5
            | CommandAction::SwitchToTab6
            | CommandAction::SwitchToTab7
            | CommandAction::SwitchToTab8
            | CommandAction::SwitchToTab9
            | CommandAction::SplitPaneVertical
            | CommandAction::SplitPaneHorizontal
            | CommandAction::ClosePane
            | CommandAction::FocusPaneLeft
            | CommandAction::FocusPaneRight
            | CommandAction::FocusPaneUp
            | CommandAction::FocusPaneDown
            | CommandAction::FocusPaneNext
            | CommandAction::FocusPanePrevious
            | CommandAction::ResizePaneLeft
            | CommandAction::ResizePaneRight
            | CommandAction::ResizePaneUp
            | CommandAction::ResizePaneDown
            | CommandAction::TogglePaneZoom => {
                self.execute_tab_command_action(action, window, cx);
            }
            CommandAction::MinimizeWindow => {
                window.minimize_window();
            }
            CommandAction::Copy | CommandAction::Paste | CommandAction::SelectAll => {
                self.execute_input_command_action(action, cx);
            }
            CommandAction::ClearScreen => {
                // Form feed is the control character produced by Ctrl+L.
                self.write_terminal_input(b"\x0c", cx);
                if self.clear_selection() {
                    cx.notify();
                }
            }
            CommandAction::ZoomIn | CommandAction::ZoomOut | CommandAction::ZoomReset => {
                self.execute_layout_command_action(action, cx);
            }
            CommandAction::OpenSearch
            | CommandAction::CloseSearch
            | CommandAction::SearchNext
            | CommandAction::SearchPrevious
            | CommandAction::ToggleSearchCaseSensitive
            | CommandAction::ToggleSearchRegex => {
                self.execute_search_command_action(action, cx);
            }
            CommandAction::InstallCli => {
                self.execute_install_cli_command_action(action, cx);
            }
        }
    }

    pub(in super::super) fn handle_toggle_command_palette_action(
        &mut self,
        _: &commands::ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ToggleCommandPalette, true, window, cx);
    }

    pub(in super::super) fn handle_import_colors_action(
        &mut self,
        _: &commands::ImportColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ImportColors, true, window, cx);
    }

    pub(in super::super) fn handle_prettify_config_action(
        &mut self,
        _: &commands::PrettifyConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::PrettifyConfig, true, window, cx);
    }

    pub(in super::super) fn handle_switch_theme_action(
        &mut self,
        _: &commands::SwitchTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchTheme, true, window, cx);
    }

    pub(in super::super) fn handle_app_info_action(
        &mut self,
        _: &commands::AppInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::AppInfo, true, window, cx);
    }

    pub(in super::super) fn handle_restart_app_action(
        &mut self,
        _: &commands::RestartApp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::RestartApp, true, window, cx);
    }

    pub(in super::super) fn handle_rename_tab_action(
        &mut self,
        _: &commands::RenameTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::RenameTab, true, window, cx);
    }

    pub(in super::super) fn handle_check_for_updates_action(
        &mut self,
        _: &commands::CheckForUpdates,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::CheckForUpdates, true, window, cx);
    }

    pub(in super::super) fn handle_toggle_tab_bar_visibility_action(
        &mut self,
        _: &commands::ToggleTabBarVisibility,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ToggleTabBarVisibility, true, window, cx);
    }

    pub(in super::super) fn handle_toggle_workspace_sidebar_action(
        &mut self,
        _: &commands::ToggleWorkspaceSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ToggleWorkspaceSidebar, true, window, cx);
    }

    pub(in super::super) fn handle_toggle_inspector_action(
        &mut self,
        _: &commands::ToggleInspector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ToggleInspector, true, window, cx);
    }

    pub(in super::super) fn handle_new_tab_action(
        &mut self,
        _: &commands::NewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::NewTab, true, window, cx);
    }

    pub(in super::super) fn handle_run_task_action(
        &mut self,
        _: &commands::RunTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::RunTask, true, window, cx);
    }

    pub(in super::super) fn handle_run_named_task_action(
        &mut self,
        action: &commands::RunNamedTask,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.simple_mode {
            return;
        }

        let Some(task) = self
            .tasks
            .iter()
            .find(|task| task.name == action.task_name)
            .cloned()
        else {
            crate::ui::toast::error(format!("Task \"{}\" no longer exists", action.task_name));
            self.notify_overlay(cx);
            return;
        };

        if let Some(layout_name) = task.layout.as_deref()
            && !self
                .current_named_layout
                .as_deref()
                .is_some_and(|current| current.eq_ignore_ascii_case(layout_name))
        {
            crate::ui::toast::info(format!(
                "Load saved layout \"{layout_name}\" before running task \"{}\"",
                task.name
            ));
            self.notify_overlay(cx);
            return;
        }

        self.run_task(
            task.name.as_str(),
            task.command.as_str(),
            task.working_dir.as_deref(),
            task.layout.as_deref(),
            cx,
        );
    }

    pub(in super::super) fn handle_run_plugin_command_action(
        &mut self,
        action: &commands::RunPluginCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_plugin_command_from_keybinding(&action.plugin_id, &action.command_id, window, cx);
    }

    pub(crate) fn open_new_tab_from_deeplink(
        &mut self,
        command: Option<&str>,
        working_dir: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = window;
        self.add_tab_with_working_dir(working_dir, cx);
        // Deeplinks may originate from browsers or documents via the OS URL
        // protocol handler, so never write deeplink-provided commands to a PTY.
        let _ = command;
    }

    pub(in super::super) fn handle_close_tab_action(
        &mut self,
        _: &commands::CloseTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::CloseTab, true, window, cx);
    }

    pub(in super::super) fn handle_close_pane_or_tab_action(
        &mut self,
        _: &commands::ClosePaneOrTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ClosePaneOrTab, true, window, cx);
    }

    pub(in super::super) fn handle_move_tab_left_action(
        &mut self,
        _: &commands::MoveTabLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::MoveTabLeft, true, window, cx);
    }

    pub(in super::super) fn handle_move_tab_right_action(
        &mut self,
        _: &commands::MoveTabRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::MoveTabRight, true, window, cx);
    }

    pub(in super::super) fn handle_switch_tab_left_action(
        &mut self,
        _: &commands::SwitchTabLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchTabLeft, true, window, cx);
    }

    pub(in super::super) fn handle_switch_tab_right_action(
        &mut self,
        _: &commands::SwitchTabRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchTabRight, true, window, cx);
    }

    pub(in super::super) fn handle_cycle_tabs_action(
        &mut self,
        _: &commands::CycleTabs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::CycleTabs, true, window, cx);
    }

    pub(in super::super) fn handle_switch_to_tab_1_action(
        &mut self,
        _: &commands::SwitchToTab1,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchToTab1, true, window, cx);
    }

    pub(in super::super) fn handle_switch_to_tab_2_action(
        &mut self,
        _: &commands::SwitchToTab2,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchToTab2, true, window, cx);
    }

    pub(in super::super) fn handle_switch_to_tab_3_action(
        &mut self,
        _: &commands::SwitchToTab3,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchToTab3, true, window, cx);
    }

    pub(in super::super) fn handle_switch_to_tab_4_action(
        &mut self,
        _: &commands::SwitchToTab4,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchToTab4, true, window, cx);
    }

    pub(in super::super) fn handle_switch_to_tab_5_action(
        &mut self,
        _: &commands::SwitchToTab5,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchToTab5, true, window, cx);
    }

    pub(in super::super) fn handle_switch_to_tab_6_action(
        &mut self,
        _: &commands::SwitchToTab6,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchToTab6, true, window, cx);
    }

    pub(in super::super) fn handle_switch_to_tab_7_action(
        &mut self,
        _: &commands::SwitchToTab7,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchToTab7, true, window, cx);
    }

    pub(in super::super) fn handle_switch_to_tab_8_action(
        &mut self,
        _: &commands::SwitchToTab8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchToTab8, true, window, cx);
    }

    pub(in super::super) fn handle_switch_to_tab_9_action(
        &mut self,
        _: &commands::SwitchToTab9,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SwitchToTab9, true, window, cx);
    }

    pub(in super::super) fn handle_manage_tmux_sessions_action(
        &mut self,
        _: &commands::ManageTmuxSessions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ManageTmuxSessions, true, window, cx);
    }

    pub(in super::super) fn handle_manage_saved_layouts_action(
        &mut self,
        _: &commands::ManageSavedLayouts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ManageSavedLayouts, true, window, cx);
    }

    pub(in super::super) fn handle_split_pane_vertical_action(
        &mut self,
        _: &commands::SplitPaneVertical,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SplitPaneVertical, true, window, cx);
    }

    pub(in super::super) fn handle_split_pane_horizontal_action(
        &mut self,
        _: &commands::SplitPaneHorizontal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SplitPaneHorizontal, true, window, cx);
    }

    pub(in super::super) fn handle_close_pane_action(
        &mut self,
        _: &commands::ClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ClosePane, true, window, cx);
    }

    pub(in super::super) fn handle_focus_pane_left_action(
        &mut self,
        _: &commands::FocusPaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::FocusPaneLeft, true, window, cx);
    }

    pub(in super::super) fn handle_focus_pane_right_action(
        &mut self,
        _: &commands::FocusPaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::FocusPaneRight, true, window, cx);
    }

    pub(in super::super) fn handle_focus_pane_up_action(
        &mut self,
        _: &commands::FocusPaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::FocusPaneUp, true, window, cx);
    }

    pub(in super::super) fn handle_focus_pane_down_action(
        &mut self,
        _: &commands::FocusPaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::FocusPaneDown, true, window, cx);
    }

    pub(in super::super) fn handle_focus_pane_next_action(
        &mut self,
        _: &commands::FocusPaneNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::FocusPaneNext, true, window, cx);
    }

    pub(in super::super) fn handle_focus_pane_previous_action(
        &mut self,
        _: &commands::FocusPanePrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::FocusPanePrevious, true, window, cx);
    }

    pub(in super::super) fn handle_resize_pane_left_action(
        &mut self,
        _: &commands::ResizePaneLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ResizePaneLeft, true, window, cx);
    }

    pub(in super::super) fn handle_resize_pane_right_action(
        &mut self,
        _: &commands::ResizePaneRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ResizePaneRight, true, window, cx);
    }

    pub(in super::super) fn handle_resize_pane_up_action(
        &mut self,
        _: &commands::ResizePaneUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ResizePaneUp, true, window, cx);
    }

    pub(in super::super) fn handle_resize_pane_down_action(
        &mut self,
        _: &commands::ResizePaneDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ResizePaneDown, true, window, cx);
    }

    pub(in super::super) fn handle_toggle_pane_zoom_action(
        &mut self,
        _: &commands::TogglePaneZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::TogglePaneZoom, true, window, cx);
    }

    pub(in super::super) fn handle_minimize_window_action(
        &mut self,
        _: &commands::MinimizeWindow,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.minimize_window();
    }

    pub(in super::super) fn handle_copy_action(
        &mut self,
        _: &commands::Copy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::Copy, true, window, cx);
    }

    pub(in super::super) fn handle_paste_action(
        &mut self,
        _: &commands::Paste,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::Paste, true, window, cx);
    }

    pub(in super::super) fn handle_clear_screen_action(
        &mut self,
        _: &commands::ClearScreen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ClearScreen, true, window, cx);
    }

    pub(in super::super) fn handle_zoom_in_action(
        &mut self,
        _: &commands::ZoomIn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ZoomIn, true, window, cx);
    }

    pub(in super::super) fn handle_zoom_out_action(
        &mut self,
        _: &commands::ZoomOut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ZoomOut, true, window, cx);
    }

    pub(in super::super) fn handle_zoom_reset_action(
        &mut self,
        _: &commands::ZoomReset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ZoomReset, true, window, cx);
    }

    pub(in super::super) fn handle_quit_action(
        &mut self,
        _: &commands::Quit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::Quit, true, window, cx);
    }

    pub(in super::super) fn handle_open_search_action(
        &mut self,
        _: &commands::OpenSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::OpenSearch, true, window, cx);
    }

    pub(in super::super) fn handle_close_search_action(
        &mut self,
        _: &commands::CloseSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::CloseSearch, true, window, cx);
    }

    pub(in super::super) fn handle_search_next_action(
        &mut self,
        _: &commands::SearchNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SearchNext, true, window, cx);
    }

    pub(in super::super) fn handle_search_previous_action(
        &mut self,
        _: &commands::SearchPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::SearchPrevious, true, window, cx);
    }

    pub(in super::super) fn handle_toggle_search_case_sensitive_action(
        &mut self,
        _: &commands::ToggleSearchCaseSensitive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ToggleSearchCaseSensitive, true, window, cx);
    }

    pub(in super::super) fn handle_toggle_search_regex_action(
        &mut self,
        _: &commands::ToggleSearchRegex,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::ToggleSearchRegex, true, window, cx);
    }

    pub(in super::super) fn handle_install_cli_action(
        &mut self,
        _: &commands::InstallCli,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.execute_command_action(CommandAction::InstallCli, true, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_input_shortcuts_allow_copy_and_paste() {
        assert!(
            TerminalView::shortcut_action_allowed_with_active_inline_input(CommandAction::Copy)
        );
        assert!(
            TerminalView::shortcut_action_allowed_with_active_inline_input(CommandAction::Paste)
        );
        assert!(
            !TerminalView::shortcut_action_allowed_with_active_inline_input(
                CommandAction::OpenSearch
            )
        );
    }

    #[test]
    fn plugin_ui_blocks_terminal_shortcuts_but_not_plugin_returned_actions() {
        assert!(TerminalView::plugin_ui_blocks_terminal_action(true, true));
        assert!(!TerminalView::plugin_ui_blocks_terminal_action(true, false));
        assert!(!TerminalView::plugin_ui_blocks_terminal_action(false, true));
    }

    #[test]
    fn switch_theme_action_maps_to_theme_palette_mode() {
        assert_eq!(
            TerminalView::command_palette_mode_for_action(CommandAction::SwitchTheme),
            Some(CommandPaletteMode::Themes)
        );
        assert_eq!(
            TerminalView::command_palette_mode_for_action(CommandAction::ManageTmuxSessions),
            Some(CommandPaletteMode::TmuxSessions)
        );
        assert_eq!(
            TerminalView::command_palette_mode_for_action(CommandAction::RunTask),
            Some(CommandPaletteMode::Tasks)
        );
        assert_eq!(
            TerminalView::command_palette_mode_for_action(CommandAction::OpenConfig),
            None
        );
        assert_eq!(
            TerminalView::command_palette_mode_for_action(CommandAction::PrettifyConfig),
            None
        );
    }

    #[test]
    fn simple_mode_blocks_palette_backed_command_actions() {
        for action in [
            CommandAction::ToggleCommandPalette,
            CommandAction::SwitchTheme,
            CommandAction::ManageTmuxSessions,
            CommandAction::ManageSavedLayouts,
            CommandAction::RunTask,
        ] {
            assert!(TerminalView::simple_mode_blocks_command_action(action));
        }

        assert!(!TerminalView::simple_mode_blocks_command_action(
            CommandAction::OpenSettings
        ));
        assert!(!TerminalView::simple_mode_blocks_command_action(
            CommandAction::OpenConfig
        ));
    }
}
