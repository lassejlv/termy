use super::*;
use std::cmp::Reverse;

fn should_show_new_tab_menu(
    runtime_kind: RuntimeKind,
    windows_shell_menu_available: bool,
    saved_ssh_hosts_available: bool,
) -> bool {
    runtime_kind == RuntimeKind::Native
        && (windows_shell_menu_available || saved_ssh_hosts_available)
}

fn runtime_config_for_windows_shell(
    base: &TerminalRuntimeConfig,
    windows_shell: RuntimeWindowsShell,
) -> TerminalRuntimeConfig {
    let mut runtime = base.clone();
    runtime.shell = None;
    runtime.windows_shell = windows_shell;
    runtime
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClosePaneOrTabTarget {
    ClosePane,
    CloseTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeSplitAxis {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeFocusDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeCloseDirection {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NativeCloseCandidate {
    direction: NativeCloseDirection,
    pane_indices: Vec<usize>,
    coverage: u16,
    required_coverage: u16,
}

impl TerminalView {
    pub(in super::super) fn execute_tab_command_action(
        &mut self,
        action: CommandAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            CommandAction::RenameTab => {
                self.begin_rename_tab(self.session.active_tab, cx);
                crate::ui::toast::info("Rename mode enabled");
                true
            }
            CommandAction::NewTab => {
                self.add_tab(cx);
                true
            }
            CommandAction::CloseTab => {
                self.request_active_tab_close(window, cx);
                true
            }
            CommandAction::ClosePaneOrTab => self.close_active_pane_or_tab(window, cx),
            CommandAction::MoveTabLeft => {
                self.move_active_tab_left(cx);
                true
            }
            CommandAction::MoveTabRight => {
                self.move_active_tab_right(cx);
                true
            }
            CommandAction::SwitchTabLeft => {
                self.switch_active_tab_left(window, cx);
                true
            }
            CommandAction::SwitchTabRight => {
                self.switch_active_tab_right(window, cx);
                true
            }
            CommandAction::CycleTabs => self.cycle_active_tab(window, cx),
            CommandAction::SwitchToTab1 => self.switch_to_tab_position(1, window, cx),
            CommandAction::SwitchToTab2 => self.switch_to_tab_position(2, window, cx),
            CommandAction::SwitchToTab3 => self.switch_to_tab_position(3, window, cx),
            CommandAction::SwitchToTab4 => self.switch_to_tab_position(4, window, cx),
            CommandAction::SwitchToTab5 => self.switch_to_tab_position(5, window, cx),
            CommandAction::SwitchToTab6 => self.switch_to_tab_position(6, window, cx),
            CommandAction::SwitchToTab7 => self.switch_to_tab_position(7, window, cx),
            CommandAction::SwitchToTab8 => self.switch_to_tab_position(8, window, cx),
            CommandAction::SwitchToTab9 => self.switch_to_tab_position(9, window, cx),
            CommandAction::SplitPaneVertical => self.split_active_pane_vertical(cx),
            CommandAction::SplitPaneHorizontal => self.split_active_pane_horizontal(cx),
            CommandAction::ClosePane => self.close_active_pane(cx),
            CommandAction::FocusPaneLeft => self.focus_pane_left(cx),
            CommandAction::FocusPaneRight => self.focus_pane_right(cx),
            CommandAction::FocusPaneUp => self.focus_pane_up(cx),
            CommandAction::FocusPaneDown => self.focus_pane_down(cx),
            CommandAction::FocusPaneNext => self.focus_pane_next(cx),
            CommandAction::FocusPanePrevious => self.focus_pane_previous(cx),
            CommandAction::ResizePaneLeft => self.resize_pane_left(cx),
            CommandAction::ResizePaneRight => self.resize_pane_right(cx),
            CommandAction::ResizePaneUp => self.resize_pane_up(cx),
            CommandAction::ResizePaneDown => self.resize_pane_down(cx),
            CommandAction::TogglePaneZoom => self.toggle_pane_zoom(cx),
            _ => false,
        }
    }

    fn switch_to_tab_position(
        &mut self,
        position: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target_index) = position.checked_sub(1) else {
            return false;
        };
        if target_index >= self.session.tabs.len() {
            return false;
        }
        self.switch_tab(target_index, cx);
        self.focus_terminal_after_tab_activation(window, cx);
        true
    }

    fn close_pane_or_tab_target(
        _runtime_kind: RuntimeKind,
        pane_count: usize,
    ) -> ClosePaneOrTabTarget {
        if pane_count > 1 {
            ClosePaneOrTabTarget::ClosePane
        } else {
            ClosePaneOrTabTarget::CloseTab
        }
    }

    fn adjacent_tab_index(active_tab: usize, tab_count: usize, to_right: bool) -> Option<usize> {
        if tab_count <= 1 || active_tab >= tab_count {
            return None;
        }

        if to_right {
            (active_tab + 1 < tab_count).then_some(active_tab + 1)
        } else {
            active_tab.checked_sub(1)
        }
    }

    fn cycled_tab_index(active_tab: usize, tab_count: usize) -> Option<usize> {
        if tab_count <= 1 || active_tab >= tab_count {
            return None;
        }

        Some((active_tab + 1) % tab_count)
    }

    fn adjacent_pane_index(active_pane: usize, pane_count: usize, step: i32) -> Option<usize> {
        if pane_count <= 1 || active_pane >= pane_count {
            return None;
        }

        if step > 0 {
            Some((active_pane + 1) % pane_count)
        } else if step < 0 {
            Some((active_pane + pane_count - 1) % pane_count)
        } else {
            None
        }
    }

    fn remap_index_after_move(index: usize, from: usize, to: usize) -> usize {
        if index == from {
            return to;
        }

        if from < to {
            if (from + 1..=to).contains(&index) {
                return index - 1;
            }
            index
        } else if (to..from).contains(&index) {
            index + 1
        } else {
            index
        }
    }

    pub(crate) fn reorder_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) -> bool {
        if from >= self.session.tabs.len() || to >= self.session.tabs.len() || from == to {
            return false;
        }

        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                if !self.tmux_reorder_tab(from, to) {
                    return false;
                }
            }
            RuntimeKind::Native => {
                let moved_tab = self.session.tabs.remove(from);
                self.session.tabs.insert(to, moved_tab);
                self.session.active_tab =
                    Self::remap_index_after_move(self.session.active_tab, from, to);
                self.renaming_tab = self
                    .renaming_tab
                    .map(|index| Self::remap_index_after_move(index, from, to));
                self.tab_strip.hovered_tab = self
                    .tab_strip
                    .hovered_tab
                    .map(|index| Self::remap_index_after_move(index, from, to));
                self.tab_strip.hovered_tab_close = self
                    .tab_strip
                    .hovered_tab_close
                    .map(|index| Self::remap_index_after_move(index, from, to));
                self.schedule_persist_native_workspace(cx);
            }
        }
        self.reset_tab_drag_state();
        self.scroll_active_tab_into_view(self.tab_strip_orientation());
        cx.notify();
        true
    }

    pub(crate) fn move_active_tab_left(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(target_index) =
            Self::adjacent_tab_index(self.session.active_tab, self.session.tabs.len(), false)
        else {
            return false;
        };

        self.reorder_tab(self.session.active_tab, target_index, cx)
    }

    pub(crate) fn move_active_tab_right(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(target_index) =
            Self::adjacent_tab_index(self.session.active_tab, self.session.tabs.len(), true)
        else {
            return false;
        };

        self.reorder_tab(self.session.active_tab, target_index, cx)
    }

    pub(crate) fn switch_active_tab_left(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target_index) =
            Self::adjacent_tab_index(self.session.active_tab, self.session.tabs.len(), false)
        else {
            return false;
        };

        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                let switched = self.tmux_switch_active_tab_left(cx);
                if switched {
                    self.sync_plugin_lifecycle_state(true, cx);
                }
                switched
            }
            RuntimeKind::Native => {
                self.switch_tab(target_index, cx);
                self.focus_terminal_after_tab_activation(window, cx);
                true
            }
        }
    }

    pub(crate) fn switch_active_tab_right(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target_index) =
            Self::adjacent_tab_index(self.session.active_tab, self.session.tabs.len(), true)
        else {
            return false;
        };

        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                let switched = self.tmux_switch_active_tab_right(cx);
                if switched {
                    self.sync_plugin_lifecycle_state(true, cx);
                }
                switched
            }
            RuntimeKind::Native => {
                self.switch_tab(target_index, cx);
                self.focus_terminal_after_tab_activation(window, cx);
                true
            }
        }
    }

    pub(crate) fn cycle_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(target_index) =
            Self::cycled_tab_index(self.session.active_tab, self.session.tabs.len())
        else {
            return false;
        };

        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                let switched = self.tmux_switch_active_tab_right(cx);
                if switched {
                    self.sync_plugin_lifecycle_state(true, cx);
                }
                switched
            }
            RuntimeKind::Native => {
                self.switch_tab(target_index, cx);
                self.focus_terminal_after_tab_activation(window, cx);
                true
            }
        }
    }

    pub(crate) fn add_tab(&mut self, cx: &mut Context<Self>) {
        let _ = self.add_tab_with_working_dir(None, cx);
    }

    /// "+" button entry point: opens a dropdown when the platform has extra
    /// tab choices; otherwise it creates a terminal tab directly.
    pub(crate) fn handle_new_tab_button(&mut self, anchor: (f32, f32), cx: &mut Context<Self>) {
        self.reload_saved_ssh_hosts();
        if should_show_new_tab_menu(
            self.runtime_kind(),
            cfg!(target_os = "windows"),
            !self.saved_ssh_hosts.is_empty(),
        ) {
            self.toggle_new_tab_menu(anchor, cx);
        } else {
            self.add_tab(cx);
        }
    }

    pub(crate) fn toggle_new_tab_menu(&mut self, anchor: (f32, f32), cx: &mut Context<Self>) {
        if self.new_tab_menu_anchor.take().is_none() {
            self.new_tab_menu_anchor = Some(anchor);
        }
        cx.notify();
        self.notify_overlay(cx);
    }

    pub(crate) fn close_new_tab_menu(&mut self, cx: &mut Context<Self>) -> bool {
        if self.new_tab_menu_anchor.take().is_some() {
            cx.notify();
            self.notify_overlay(cx);
            true
        } else {
            false
        }
    }

    pub(in super::super) fn reload_saved_ssh_hosts(&mut self) {
        match crate::ssh::load_hosts(self.config_path.as_deref()) {
            Ok(hosts) => self.saved_ssh_hosts = hosts,
            Err(error) => {
                log::warn!("Failed to reload saved SSH hosts: {error}");
                crate::ui::toast::error(error);
                self.saved_ssh_hosts.clear();
            }
        }
    }

    pub(crate) fn add_tab_with_working_dir(
        &mut self,
        working_dir: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.add_tab_with_working_dir_and_windows_shell(working_dir, None, None, cx)
    }

    pub(crate) fn add_tab_with_launch(
        &mut self,
        working_dir: Option<&str>,
        launch: Option<&TerminalLaunch>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.add_tab_with_working_dir_and_windows_shell(working_dir, None, launch, cx)
    }

    pub(crate) fn add_tab_with_windows_shell(
        &mut self,
        windows_shell: RuntimeWindowsShell,
        cx: &mut Context<Self>,
    ) -> bool {
        self.add_tab_with_working_dir_and_windows_shell(None, Some(windows_shell), None, cx)
    }

    fn add_tab_with_working_dir_and_windows_shell(
        &mut self,
        working_dir: Option<&str>,
        windows_shell: Option<RuntimeWindowsShell>,
        launch: Option<&TerminalLaunch>,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                if matches!(launch, Some(TerminalLaunch::Program { .. })) {
                    crate::ui::toast::error(
                        "Structured program launches are not supported in tmux tabs",
                    );
                    return false;
                }
                let added = self.tmux_add_tab(working_dir, cx);
                if added {
                    if let Some(TerminalLaunch::ShellCommand(command)) = launch {
                        let mut input = command.as_bytes().to_vec();
                        input.push(b'\n');
                        if !self.send_input_to_active_pane(&input) {
                            crate::ui::toast::error(
                                "Failed to send the plugin command to the new tmux tab",
                            );
                            return false;
                        }
                    }
                    self.sync_plugin_lifecycle_state(true, cx);
                }
                added
            }
            RuntimeKind::Native => {
                // Tab creation should stay robust if active pane state is transiently missing.
                let size = self
                    .active_terminal()
                    .map(|terminal| terminal.size())
                    .unwrap_or_default();
                let preferred_working_dir =
                    self.preferred_working_dir_for_new_session(working_dir, cx);
                let terminal_runtime = windows_shell
                    .map(|shell| runtime_config_for_windows_shell(&self.terminal_runtime, shell));
                let terminal_runtime = terminal_runtime.as_ref().unwrap_or(&self.terminal_runtime);
                let terminal = match Terminal::new_native_with_launch(
                    size,
                    preferred_working_dir.as_deref(),
                    Some(&self.native_terminal_wakeup_router),
                    Some(&self.tab_shell_integration),
                    Some(terminal_runtime),
                    launch,
                ) {
                    Ok(terminal) => terminal,
                    Err(error) => {
                        crate::ui::toast::error(format!("Failed to create tab: {error}"));
                        return false;
                    }
                };

                let predicted_prompt_cwd = Self::predicted_prompt_cwd(
                    preferred_working_dir.as_deref(),
                    self.terminal_runtime.working_dir_fallback,
                );
                let predicted_title = Self::predicted_prompt_seed_title(
                    &self.tab_title,
                    predicted_prompt_cwd.as_deref(),
                );

                self.insert_native_terminal_tab(terminal, size, predicted_title, cx)
            }
        }
    }

    pub(crate) fn add_ssh_tab(&mut self, host_id: &str, cx: &mut Context<Self>) -> bool {
        if self.runtime_kind() != RuntimeKind::Native {
            crate::ui::toast::error(
                "SSH hosts can only be opened from the native terminal runtime",
            );
            return false;
        }

        self.reload_saved_ssh_hosts();
        let Some(host) = self
            .saved_ssh_hosts
            .iter()
            .find(|host| host.id == host_id)
            .cloned()
        else {
            crate::ui::toast::error("That saved SSH host no longer exists");
            return false;
        };
        let process = match termy_ssh_core::openssh_launch(&host) {
            Ok(process) => process,
            Err(error) => {
                crate::ui::toast::error(format!(
                    "Invalid SSH host “{}”: {error}",
                    host.display_name
                ));
                return false;
            }
        };

        let mut runtime_config = self.terminal_runtime.clone();
        let saved_secret_available = match crate::ssh::manager(self.config_path.as_deref())
            .and_then(|manager| manager.has_secret(&host.id, host.authentication.secret_kind()))
        {
            Ok(available) => available,
            Err(error) => {
                crate::ui::toast::warning(format!(
                    "Could not read the saved credential for “{}”; SSH will prompt in the terminal: {error}",
                    host.display_name
                ));
                false
            }
        };
        if saved_secret_available {
            let askpass = std::env::current_exe()
                .map_err(|error| format!("Unable to locate the Termy executable: {error}"))
                .and_then(|executable| {
                    termy_ssh_core::askpass_environment(&executable, std::process::id(), &host)
                });
            match askpass {
                Ok(environment) => runtime_config.environment.extend(environment),
                Err(error) => {
                    crate::ui::toast::warning(format!(
                        "Could not prepare the saved credential for “{}”; SSH will prompt in the terminal: {error}",
                        host.display_name
                    ));
                }
            }
        }

        let size = self
            .active_terminal()
            .map(|terminal| terminal.size())
            .unwrap_or_default();
        let launch = TerminalLaunch::Program {
            program: process.program,
            args: process.args,
        };
        let terminal = match Terminal::new_native_with_launch(
            size,
            None,
            Some(&self.native_terminal_wakeup_router),
            Some(&self.tab_shell_integration),
            Some(&runtime_config),
            Some(&launch),
        ) {
            Ok(terminal) => terminal,
            Err(error) => {
                crate::ui::toast::error(format!(
                    "Could not start SSH session “{}”: {error}. Check that OpenSSH is installed and ssh is on PATH.",
                    host.display_name
                ));
                return false;
            }
        };

        self.insert_native_terminal_tab(terminal, size, Some(host.display_name), cx)
    }

    fn insert_native_terminal_tab(
        &mut self,
        terminal: Terminal,
        size: TerminalSize,
        predicted_title: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let tab_id = self.allocate_tab_id();
        let old_active_tab = self.session.active_tab;
        let new_tab_index = self
            .session
            .active_tab
            .saturating_add(1)
            .min(self.session.tabs.len());
        self.session.tabs.insert(
            new_tab_index,
            Self::create_native_tab(tab_id, terminal, size.cols, size.rows, predicted_title),
        );
        self.refresh_tab_title(new_tab_index);
        self.session.active_tab = new_tab_index;
        if let Some(inactive_scrollback) = self.inactive_tab_scrollback {
            let active_options = self.terminal_runtime.term_options();
            let inactive_options = active_options.with_scrollback_history(inactive_scrollback);
            if let Some(tab) = self.session.tabs.get(old_active_tab) {
                for pane in &tab.panes {
                    pane.terminal().set_term_options(inactive_options);
                }
            }
            if let Some(tab) = self.session.tabs.get(new_tab_index) {
                for pane in &tab.panes {
                    pane.terminal().set_term_options(active_options);
                }
            }
        }
        self.mark_tab_strip_layout_dirty();
        self.reset_tab_interaction_state();
        self.sync_tab_strip_for_active_tab();
        self.sync_plugin_lifecycle_state(false, cx);
        self.schedule_persist_native_workspace(cx);
        self.start_new_tab_animation(tab_id, cx);
        cx.notify();
        true
    }

    pub(crate) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.session.tabs.len() || self.session.tabs[index].pinned {
            return;
        }
        let removed_tab_id = self.session.tabs[index].id;
        let removed_pane_ids = self.session.tabs[index]
            .panes
            .iter()
            .map(|pane| pane.id.clone())
            .collect::<Vec<_>>();
        let _ = self.release_forwarded_mouse_presses_for_panes(&removed_pane_ids);

        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                self.tmux_close_tab(index, cx);
                self.sync_plugin_lifecycle_state(true, cx);
                return;
            }
            RuntimeKind::Native => {}
        };

        self.session.tabs.remove(index);
        self.session
            .native_pane_zoom_snapshots
            .remove(&removed_tab_id);
        self.session
            .native_pane_layout_trees
            .remove(&removed_tab_id);
        self.mark_tab_strip_layout_dirty();

        if self.session.tabs.is_empty() {
            self.session.active_tab = 0;
        } else if self.session.active_tab > index {
            self.session.active_tab -= 1;
        } else if self.session.active_tab >= self.session.tabs.len() {
            self.session.active_tab = self.session.tabs.len() - 1;
        }

        match self.renaming_tab {
            Some(editing) if editing == index => {
                self.reset_tab_rename_state();
            }
            Some(editing) if editing > index => {
                self.renaming_tab = Some(editing - 1);
            }
            _ => {}
        }

        self.tab_strip.hovered_tab = match self.tab_strip.hovered_tab {
            Some(hovered) if hovered == index => None,
            Some(hovered) if hovered > index => Some(hovered - 1),
            value => value,
        };
        self.tab_strip.hovered_tab_close = match self.tab_strip.hovered_tab_close {
            Some(hovered) if hovered == index => None,
            Some(hovered) if hovered > index => Some(hovered - 1),
            value => value,
        };
        self.reset_tab_drag_state();

        self.clear_selection();
        self.sync_tab_strip_for_active_tab();
        self.sync_plugin_lifecycle_state(false, cx);
        self.schedule_persist_native_workspace(cx);
        cx.notify();
    }

    pub(crate) fn tab_index_by_id(&self, tab_id: TabId) -> Option<usize> {
        self.session.tabs.iter().position(|tab| tab.id == tab_id)
    }

    pub(crate) fn set_tab_pinned(
        &mut self,
        index: usize,
        pinned: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.session.tabs.get_mut(index) else {
            return false;
        };
        if tab.pinned == pinned {
            return false;
        }

        tab.pinned = pinned;
        self.mark_tab_strip_layout_dirty();
        if self.runtime_kind() == RuntimeKind::Native {
            self.schedule_persist_native_workspace(cx);
        }
        cx.notify();
        true
    }

    pub(crate) fn set_tab_pinned_by_id(
        &mut self,
        tab_id: TabId,
        pinned: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.tab_index_by_id(tab_id) else {
            return false;
        };
        self.set_tab_pinned(index, pinned, cx)
    }

    pub(crate) fn begin_rename_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.session.tabs.len() {
            return;
        }

        if self.is_command_palette_open() {
            self.close_command_palette(cx);
        }
        if self.search_open {
            self.close_search(cx);
        }

        self.reset_workspace_rename_state();
        if self.session.active_tab != index {
            self.switch_tab(index, cx);
        }

        self.reset_tab_drag_state();
        self.renaming_tab = Some(index);
        self.rename_input
            .set_text(self.session.tabs[index].title.clone());
        self.reset_cursor_blink_phase();
        self.inline_input_selecting = false;
        // Recompute tab widths so the renamed tab expands to its editing width.
        self.mark_tab_strip_layout_dirty();
        cx.notify();
    }

    pub(crate) fn switch_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.session.tabs.len() || index == self.session.active_tab {
            return;
        }

        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                self.tmux_switch_tab(index, cx);
                self.sync_plugin_lifecycle_state(true, cx);
            }
            RuntimeKind::Native => {
                let old_active = self.session.active_tab;
                self.session.active_tab = index;
                if !matches!(
                    self.tab_width_mode,
                    TabWidthMode::Stable | TabWidthMode::Uniform
                ) {
                    self.mark_tab_strip_layout_dirty();
                }

                if let Some(inactive_scrollback) = self.inactive_tab_scrollback {
                    let active_options = self.terminal_runtime.term_options();
                    let inactive_options =
                        active_options.with_scrollback_history(inactive_scrollback);
                    for pane in &self.session.tabs[old_active].panes {
                        pane.terminal().set_term_options(inactive_options);
                    }
                    for pane in &self.session.tabs[index].panes {
                        pane.terminal().set_term_options(active_options);
                    }
                }

                self.reset_tab_rename_state();
                self.reset_tab_drag_state();
                self.clear_selection();
                self.sync_tab_strip_for_active_tab();
                self.sync_plugin_lifecycle_state(false, cx);
                self.schedule_persist_native_workspace(cx);
                cx.notify();
            }
        }
    }

    pub(crate) fn focus_terminal_after_tab_activation(
        &mut self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window);
        self.reset_cursor_blink_phase();
    }

    pub(crate) fn commit_rename_tab(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.renaming_tab else {
            return;
        };

        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                self.tmux_commit_rename_tab(index);
            }
            RuntimeKind::Native => {
                let trimmed = self.rename_input.text().trim();
                self.session.tabs[index].manual_title = (!trimmed.is_empty())
                    .then(|| Self::truncate_tab_title(trimmed))
                    .filter(|title| !title.is_empty());
                self.refresh_tab_title(index);
                self.schedule_persist_native_workspace(cx);
            }
        }

        self.reset_tab_rename_state();
        self.reset_tab_drag_state();
        cx.notify();
    }

    pub(crate) fn cancel_rename_tab(&mut self, cx: &mut Context<Self>) {
        if self.renaming_tab.is_none() {
            return;
        }

        self.reset_tab_rename_state();
        self.reset_tab_drag_state();
        cx.notify();
    }

    pub(crate) fn focus_pane_target(&mut self, pane_id: &str, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_focus_pane_target(pane_id, cx),
            RuntimeKind::Native => self.native_focus_pane_target(pane_id, cx),
        }
    }

    pub(crate) fn split_active_pane_vertical(&mut self, cx: &mut Context<Self>) -> bool {
        self.split_active_pane_vertical_with_launch(None, None, cx)
    }

    pub(crate) fn split_active_pane_vertical_with_launch(
        &mut self,
        working_dir: Option<&str>,
        launch: Option<&TerminalLaunch>,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                if matches!(launch, Some(TerminalLaunch::Program { .. })) {
                    crate::ui::toast::error(
                        "Structured program launches are not supported in tmux panes",
                    );
                    return false;
                }
                let split = self.tmux_split_active_pane_vertical_with_working_dir(working_dir, cx);
                split && self.send_shell_launch_to_active_pane(launch)
            }
            RuntimeKind::Native => {
                self.native_split_active_pane(NativeSplitAxis::Vertical, working_dir, launch, cx)
            }
        }
    }

    pub(crate) fn split_active_pane_horizontal(&mut self, cx: &mut Context<Self>) -> bool {
        self.split_active_pane_horizontal_with_launch(None, None, cx)
    }

    pub(crate) fn split_active_pane_horizontal_with_launch(
        &mut self,
        working_dir: Option<&str>,
        launch: Option<&TerminalLaunch>,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => {
                if matches!(launch, Some(TerminalLaunch::Program { .. })) {
                    crate::ui::toast::error(
                        "Structured program launches are not supported in tmux panes",
                    );
                    return false;
                }
                let split =
                    self.tmux_split_active_pane_horizontal_with_working_dir(working_dir, cx);
                split && self.send_shell_launch_to_active_pane(launch)
            }
            RuntimeKind::Native => {
                self.native_split_active_pane(NativeSplitAxis::Horizontal, working_dir, launch, cx)
            }
        }
    }

    fn send_shell_launch_to_active_pane(&self, launch: Option<&TerminalLaunch>) -> bool {
        let Some(TerminalLaunch::ShellCommand(command)) = launch else {
            return true;
        };
        let mut input = command.as_bytes().to_vec();
        input.push(b'\n');
        self.send_input_to_active_pane(&input)
    }

    pub(crate) fn close_active_pane(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(active_pane_id) = self.active_pane_id().map(str::to_string) {
            let _ = self
                .release_forwarded_mouse_presses_for_panes(std::slice::from_ref(&active_pane_id));
        }
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_close_active_pane(cx),
            RuntimeKind::Native => self.native_close_active_pane(cx),
        }
    }

    pub(crate) fn close_native_pane_by_id(
        &mut self,
        tab_id: TabId,
        pane_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.runtime_kind() != RuntimeKind::Native {
            return false;
        }
        let Some(tab_index) = self.tab_index_by_id(tab_id) else {
            return false;
        };
        let Some(tab) = self.session.tabs.get(tab_index) else {
            return false;
        };
        if tab.panes.len() <= 1 {
            return false;
        }
        if !tab.panes.iter().any(|pane| pane.id == pane_id) {
            return false;
        }

        let pane_id = pane_id.to_string();
        let _ = self.release_forwarded_mouse_presses_for_panes(std::slice::from_ref(&pane_id));
        self.clear_native_zoom_snapshot_for_tab_id(tab_id);

        if self.ensure_native_layout_tree_for_tab_id(tab_id)
            && let Some(tree) = self.session.native_pane_layout_trees.remove(&tab_id)
        {
            let (next_root, next_focus_id, removed) =
                Self::native_remove_leaf_from_tree(tree.root, pane_id.as_str());
            if removed && let Some(next_root) = next_root {
                self.session
                    .native_pane_layout_trees
                    .insert(tab_id, NativePaneLayoutTree { root: next_root });
                let (cols, rows) = if let Some(tab) = self.session.tabs.get_mut(tab_index) {
                    let prev_cols = tab
                        .panes
                        .iter()
                        .map(|pane| pane.left.saturating_add(pane.width))
                        .max()
                        .unwrap_or(1)
                        .max(1);
                    let prev_rows = tab
                        .panes
                        .iter()
                        .map(|pane| pane.top.saturating_add(pane.height))
                        .max()
                        .unwrap_or(1)
                        .max(1);
                    tab.panes.retain(|pane| pane.id != pane_id);
                    if tab.active_pane_id == pane_id || !tab.has_active_pane() {
                        tab.active_pane_id = next_focus_id
                            .or_else(|| tab.panes.first().map(|pane| pane.id.clone()))
                            .unwrap_or_default();
                    }
                    if tab.panes.len() == 1
                        && let Some(remaining_pane) = tab.panes.first_mut()
                    {
                        remaining_pane.pane_zoom_steps = 0;
                    }
                    tab.assert_active_pane_invariant();
                    (prev_cols, prev_rows)
                } else {
                    return false;
                };

                self.apply_native_layout_tree_to_tab(tab_id, cols, rows);
                if tab_index == self.session.active_tab {
                    self.clear_selection();
                    self.clear_hovered_link();
                    self.clear_terminal_scrollbar_marker_cache();
                }
                self.schedule_persist_native_workspace(cx);
                cx.notify();
                return true;
            }
        }

        let Some(tab) = self.session.tabs.get_mut(tab_index) else {
            return false;
        };
        let Some(removed_index) = tab.panes.iter().position(|pane| pane.id == pane_id) else {
            return false;
        };
        let removed = tab.panes.remove(removed_index);
        Self::native_close_expand_neighbors(&mut tab.panes, &removed);

        if tab.active_pane_id == pane_id || !tab.has_active_pane() {
            let next_index = removed_index.min(tab.panes.len().saturating_sub(1));
            if let Some(next) = tab.panes.get(next_index) {
                tab.active_pane_id = next.id.clone();
            }
        }
        if tab.panes.len() == 1
            && let Some(remaining_pane) = tab.panes.first_mut()
        {
            remaining_pane.pane_zoom_steps = 0;
        }
        tab.assert_active_pane_invariant();

        if tab_index == self.session.active_tab {
            self.clear_selection();
            self.clear_hovered_link();
            self.clear_terminal_scrollbar_marker_cache();
        }
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    pub(crate) fn close_active_pane_or_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let pane_count = self
            .session
            .tabs
            .get(self.session.active_tab)
            .map_or(0, |tab| tab.panes.len());
        match Self::close_pane_or_tab_target(self.runtime_kind(), pane_count) {
            ClosePaneOrTabTarget::ClosePane => self.close_active_pane(cx),
            ClosePaneOrTabTarget::CloseTab => {
                // tmux rejects killing the last pane in a window, so we intentionally
                // promote that case to the existing tab-close flow.
                self.request_active_tab_close(window, cx);
                true
            }
        }
    }

    pub(crate) fn focus_pane_left(&mut self, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_focus_pane_left(cx),
            RuntimeKind::Native => self.native_focus_pane_direction(NativeFocusDirection::Left, cx),
        }
    }

    pub(crate) fn focus_pane_right(&mut self, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_focus_pane_right(cx),
            RuntimeKind::Native => {
                self.native_focus_pane_direction(NativeFocusDirection::Right, cx)
            }
        }
    }

    pub(crate) fn focus_pane_up(&mut self, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_focus_pane_up(cx),
            RuntimeKind::Native => self.native_focus_pane_direction(NativeFocusDirection::Up, cx),
        }
    }

    pub(crate) fn focus_pane_down(&mut self, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_focus_pane_down(cx),
            RuntimeKind::Native => self.native_focus_pane_direction(NativeFocusDirection::Down, cx),
        }
    }

    fn focus_pane_cycle(&mut self, step: i32, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.session.tabs.get(self.session.active_tab) else {
            return false;
        };
        let Some(active_pane_index) = tab.active_pane_index() else {
            return false;
        };
        let Some(target_pane_index) =
            Self::adjacent_pane_index(active_pane_index, tab.panes.len(), step)
        else {
            return false;
        };

        let target_pane_id = tab.panes[target_pane_index].id.clone();
        self.focus_pane_target(target_pane_id.as_str(), cx)
    }

    pub(crate) fn focus_pane_next(&mut self, cx: &mut Context<Self>) -> bool {
        self.focus_pane_cycle(1, cx)
    }

    pub(crate) fn focus_pane_previous(&mut self, cx: &mut Context<Self>) -> bool {
        self.focus_pane_cycle(-1, cx)
    }

    pub(crate) fn resize_pane_left(&mut self, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_resize_pane_left(cx),
            RuntimeKind::Native => self.native_resize_active_pane(
                PaneResizeAxis::Horizontal,
                PaneResizeEdge::Left,
                -1,
                cx,
            ),
        }
    }

    pub(crate) fn resize_pane_right(&mut self, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_resize_pane_right(cx),
            RuntimeKind::Native => self.native_resize_active_pane(
                PaneResizeAxis::Horizontal,
                PaneResizeEdge::Right,
                1,
                cx,
            ),
        }
    }

    pub(crate) fn resize_pane_up(&mut self, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_resize_pane_up(cx),
            RuntimeKind::Native => self.native_resize_active_pane(
                PaneResizeAxis::Vertical,
                PaneResizeEdge::Top,
                -1,
                cx,
            ),
        }
    }

    pub(crate) fn resize_pane_down(&mut self, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_resize_pane_down(cx),
            RuntimeKind::Native => self.native_resize_active_pane(
                PaneResizeAxis::Vertical,
                PaneResizeEdge::Bottom,
                1,
                cx,
            ),
        }
    }

    pub(crate) fn toggle_pane_zoom(&mut self, cx: &mut Context<Self>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_toggle_active_pane_zoom(cx),
            RuntimeKind::Native => self.native_toggle_active_pane_zoom(cx),
        }
    }

    pub(in super::super) fn clear_native_zoom_snapshot_for_active_tab(&mut self) {
        if let Some(tab) = self.session.tabs.get(self.session.active_tab) {
            self.clear_native_zoom_snapshot_for_tab_id(tab.id);
        }
    }

    fn clear_native_zoom_snapshot_for_tab_id(&mut self, tab_id: TabId) {
        self.session.native_pane_zoom_snapshots.remove(&tab_id);
    }

    fn native_resize_active_pane(
        &mut self,
        axis: PaneResizeAxis,
        edge: PaneResizeEdge,
        divider_delta: i16,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(active_pane_id) = self.active_pane_id().map(ToOwned::to_owned) else {
            return false;
        };
        self.clear_native_zoom_snapshot_for_active_tab();
        if self.native_resize_pane_step(active_pane_id.as_str(), axis, edge, divider_delta)
            != PaneResizeResult::Applied
        {
            return false;
        }
        self.clear_selection();
        self.clear_hovered_link();
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    fn native_toggle_active_pane_zoom(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(tab_id) = self
            .session
            .tabs
            .get(self.session.active_tab)
            .map(|tab| tab.id)
        else {
            return false;
        };

        if let Some(snapshot) = self.session.native_pane_zoom_snapshots.remove(&tab_id) {
            let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) else {
                return false;
            };
            let Some(mut active_pane) = tab.panes.pop() else {
                return false;
            };
            active_pane.left = snapshot.active_pane_geometry.0;
            active_pane.top = snapshot.active_pane_geometry.1;
            active_pane.width = snapshot.active_pane_geometry.2;
            active_pane.height = snapshot.active_pane_geometry.3;

            let mut panes = snapshot.other_panes;
            let insert_index = snapshot.active_original_index.min(panes.len());
            panes.insert(insert_index, active_pane);
            tab.panes = panes;
            tab.active_pane_id = snapshot.active_pane_id;
            tab.assert_active_pane_invariant();
            if let Some(layout_tree) = snapshot.layout_tree {
                self.session
                    .native_pane_layout_trees
                    .insert(tab_id, layout_tree);
            }

            self.clear_selection();
            self.clear_hovered_link();
            self.evict_inactive_terminal_render_caches();
            self.sync_native_terminal_wakeup_interest();
            self.schedule_persist_native_workspace(cx);
            cx.notify();
            return true;
        }

        let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) else {
            return false;
        };
        if tab.panes.len() <= 1 {
            return false;
        }
        let active_pane_id = tab.active_pane_id.clone();
        let Some(active_index) = tab.active_pane_index() else {
            return false;
        };

        let max_cols = tab
            .panes
            .iter()
            .map(|pane| pane.left.saturating_add(pane.width))
            .max()
            .unwrap_or(1)
            .max(1);
        let max_rows = tab
            .panes
            .iter()
            .map(|pane| pane.top.saturating_add(pane.height))
            .max()
            .unwrap_or(1)
            .max(1);

        let mut panes = std::mem::take(&mut tab.panes);
        let mut active_pane = panes.remove(active_index);
        let active_geometry = (
            active_pane.left,
            active_pane.top,
            active_pane.width,
            active_pane.height,
        );
        active_pane.left = 0;
        active_pane.top = 0;
        active_pane.width = max_cols;
        active_pane.height = max_rows;
        tab.panes = vec![active_pane];
        tab.active_pane_id = active_pane_id.clone();
        tab.assert_active_pane_invariant();
        let layout_tree = self.session.native_pane_layout_trees.remove(&tab_id);

        self.session.native_pane_zoom_snapshots.insert(
            tab_id,
            NativePaneZoomSnapshot {
                other_panes: panes,
                active_pane_geometry: active_geometry,
                active_pane_id,
                active_original_index: active_index,
                layout_tree,
            },
        );

        self.clear_selection();
        self.clear_hovered_link();
        self.evict_inactive_terminal_render_caches();
        self.sync_native_terminal_wakeup_interest();
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    fn native_allocate_pane_id(&self) -> String {
        let mut next = 1u64;
        loop {
            let candidate = format!("%native-pane-{next}");
            if self.pane_terminal_by_id(candidate.as_str()).is_none() {
                return candidate;
            }
            next = next.saturating_add(1);
        }
    }

    fn native_make_terminal(
        &mut self,
        cols: u16,
        rows: u16,
        cell_size: Size<Pixels>,
        working_dir: Option<&str>,
        launch: Option<&TerminalLaunch>,
        cx: &mut Context<Self>,
    ) -> Result<Terminal, String> {
        let preferred_working_dir = self.preferred_working_dir_for_new_session(working_dir, cx);
        Terminal::new_native_with_launch(
            TerminalSize {
                cols: cols.max(1),
                rows: rows.max(1),
                cell_width: cell_size.width.into(),
                cell_height: cell_size.height.into(),
            },
            preferred_working_dir.as_deref(),
            Some(&self.native_terminal_wakeup_router),
            Some(&self.tab_shell_integration),
            Some(&self.terminal_runtime),
            launch,
        )
        .map_err(|error| format!("Failed to split pane: {error}"))
    }

    fn native_focus_pane_target(&mut self, pane_id: &str, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) else {
            return false;
        };
        if tab.active_pane_id == pane_id {
            return false;
        }
        if !tab.panes.iter().any(|pane| pane.id == pane_id) {
            return false;
        }

        tab.active_pane_id = pane_id.to_string();
        tab.assert_active_pane_invariant();
        self.clear_selection();
        self.clear_hovered_link();
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    fn native_split_active_pane(
        &mut self,
        axis: NativeSplitAxis,
        working_dir: Option<&str>,
        launch: Option<&TerminalLaunch>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.clear_native_zoom_snapshot_for_active_tab();
        let Some((active_pane_id, left, top, width, height, pane_zoom_steps)) = self
            .session
            .tabs
            .get(self.session.active_tab)
            .and_then(|tab| {
                let index = tab.active_pane_index()?;
                let pane = tab.panes.get(index)?;
                Some((
                    pane.id.clone(),
                    pane.left,
                    pane.top,
                    pane.width,
                    pane.height,
                    pane.pane_zoom_steps,
                ))
            })
        else {
            return false;
        };

        let (current_size, split_size) = match axis {
            NativeSplitAxis::Vertical => {
                let min_width = Self::native_pane_min_extent_for_axis(PaneResizeAxis::Horizontal);
                if width < min_width.saturating_mul(2) {
                    crate::ui::toast::info(format!(
                        "Pane needs at least {} columns to split vertically",
                        min_width.saturating_mul(2)
                    ));
                    self.notify_overlay(cx);
                    return false;
                }
                let current_width = (width / 2).max(min_width);
                let split_width = width.saturating_sub(current_width).max(min_width);
                (
                    (left, top, current_width, height),
                    (left.saturating_add(current_width), top, split_width, height),
                )
            }
            NativeSplitAxis::Horizontal => {
                let min_height = Self::native_pane_min_extent_for_axis(PaneResizeAxis::Vertical);
                if height < min_height.saturating_mul(2) {
                    crate::ui::toast::info(format!(
                        "Pane needs at least {} rows to split horizontally",
                        min_height.saturating_mul(2)
                    ));
                    self.notify_overlay(cx);
                    return false;
                }
                let current_height = (height / 2).max(min_height);
                let split_height = height.saturating_sub(current_height).max(min_height);
                (
                    (left, top, width, current_height),
                    (
                        left,
                        top.saturating_add(current_height),
                        width,
                        split_height,
                    ),
                )
            }
        };

        let cell_size = self.layout_cell_size();
        let terminal = match self.native_make_terminal(
            split_size.2,
            split_size.3,
            cell_size,
            working_dir,
            launch,
            cx,
        ) {
            Ok(terminal) => terminal,
            Err(error) => {
                crate::ui::toast::error(error);
                return false;
            }
        };
        let pane_id = self.native_allocate_pane_id();
        let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) else {
            return false;
        };
        let Some(active_index) = tab.panes.iter().position(|pane| pane.id == active_pane_id) else {
            return false;
        };

        if let Some(active_pane) = tab.panes.get_mut(active_index) {
            active_pane.left = current_size.0;
            active_pane.top = current_size.1;
            active_pane.width = current_size.2;
            active_pane.height = current_size.3;
            // Resize terminal before the first render so pane geometry and grid size match.
            active_pane.terminal_mut().resize(TerminalSize {
                cols: current_size.2,
                rows: current_size.3,
                cell_width: cell_size.width.into(),
                cell_height: cell_size.height.into(),
            });
        }

        let cached_element_ids = PaneCachedElementIds::new(&pane_id);
        let split_pane = TerminalPane {
            id: pane_id.clone(),
            left: split_size.0,
            top: split_size.1,
            width: split_size.2,
            height: split_size.3,
            pane_zoom_steps,
            degraded: false,
            tmux_mouse_mode: None,
            progress_state: ProgressState::default(),
            terminal,
            render_cache: RefCell::new(TerminalPaneRenderCache::default()),
            last_alternate_screen: Cell::new(false),
            cached_element_ids,
        };

        tab.panes.insert(active_index + 1, split_pane);
        tab.active_pane_id = pane_id.clone();
        tab.assert_active_pane_invariant();
        let tab_id = tab.id;
        let max_cols = tab
            .panes
            .iter()
            .map(|pane| pane.left.saturating_add(pane.width))
            .max()
            .unwrap_or(split_size.2)
            .max(1);
        let max_rows = tab
            .panes
            .iter()
            .map(|pane| pane.top.saturating_add(pane.height))
            .max()
            .unwrap_or(split_size.3)
            .max(1);
        if self.ensure_native_layout_tree_for_tab_id(tab_id)
            && let Some(tree) = self.session.native_pane_layout_trees.get_mut(&tab_id)
        {
            let layout_axis = match axis {
                NativeSplitAxis::Vertical => PaneResizeAxis::Horizontal,
                NativeSplitAxis::Horizontal => PaneResizeAxis::Vertical,
            };
            if Self::native_replace_leaf_with_split(
                &mut tree.root,
                active_pane_id.as_str(),
                layout_axis,
                pane_id.as_str(),
            ) {
                Self::native_balance_split_group_containing_leaf(
                    &mut tree.root,
                    layout_axis,
                    pane_id.as_str(),
                );
            }
            self.apply_native_layout_tree_to_tab(tab_id, max_cols, max_rows);
        }
        self.clear_selection();
        self.clear_hovered_link();
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    fn native_overlap_cells(a_start: u16, a_end: u16, b_start: u16, b_end: u16) -> u16 {
        let start = a_start.max(b_start);
        let end = a_end.min(b_end);
        end.saturating_sub(start)
    }

    fn native_pane_rect_from_pane(pane: &TerminalPane) -> NativePaneRect {
        NativePaneRect {
            left: pane.left,
            top: pane.top,
            width: pane.width,
            height: pane.height,
        }
    }

    fn native_pane_rects_overlap(a: NativePaneRect, b: NativePaneRect) -> bool {
        let a_right = a.left.saturating_add(a.width);
        let a_bottom = a.top.saturating_add(a.height);
        let b_right = b.left.saturating_add(b.width);
        let b_bottom = b.top.saturating_add(b.height);
        a.left < b_right && b.left < a_right && a.top < b_bottom && b.top < a_bottom
    }

    fn native_close_direction_coverage(
        mut intervals: Vec<(u16, u16)>,
        target_start: u16,
        target_end: u16,
    ) -> u16 {
        if intervals.is_empty() || target_start >= target_end {
            return 0;
        }

        intervals.sort_unstable_by_key(|&(start, end)| (start, end));
        let mut coverage = 0u16;
        let mut current = intervals[0];

        for interval in intervals.into_iter().skip(1) {
            if interval.0 <= current.1 {
                current.1 = current.1.max(interval.1);
                continue;
            }

            coverage = coverage.saturating_add(current.1.saturating_sub(current.0));
            current = interval;
        }

        coverage
            .saturating_add(current.1.saturating_sub(current.0))
            .min(target_end.saturating_sub(target_start))
    }

    fn native_close_expand_pane_rects(
        pane_rects: &mut [NativePaneRect],
        pane_indices: &[usize],
        removed: &TerminalPane,
        direction: NativeCloseDirection,
    ) {
        let removed_width = removed.width;
        let removed_height = removed.height;

        match direction {
            NativeCloseDirection::Left => {
                for &index in pane_indices {
                    pane_rects[index].width = pane_rects[index].width.saturating_add(removed_width);
                }
            }
            NativeCloseDirection::Right => {
                for &index in pane_indices {
                    pane_rects[index].left = pane_rects[index].left.saturating_sub(removed_width);
                    pane_rects[index].width = pane_rects[index].width.saturating_add(removed_width);
                }
            }
            NativeCloseDirection::Top => {
                for &index in pane_indices {
                    pane_rects[index].height =
                        pane_rects[index].height.saturating_add(removed_height);
                }
            }
            NativeCloseDirection::Bottom => {
                for &index in pane_indices {
                    pane_rects[index].top = pane_rects[index].top.saturating_sub(removed_height);
                    pane_rects[index].height =
                        pane_rects[index].height.saturating_add(removed_height);
                }
            }
        }
    }

    fn native_close_direction_preserves_layout(
        panes: &[TerminalPane],
        removed: &TerminalPane,
        candidate: &NativeCloseCandidate,
    ) -> bool {
        if candidate.pane_indices.is_empty() {
            return false;
        }

        let mut pane_rects = panes
            .iter()
            .map(Self::native_pane_rect_from_pane)
            .collect::<Vec<_>>();
        Self::native_close_expand_pane_rects(
            &mut pane_rects,
            &candidate.pane_indices,
            removed,
            candidate.direction,
        );

        for left_index in 0..pane_rects.len() {
            for right_index in left_index + 1..pane_rects.len() {
                if Self::native_pane_rects_overlap(pane_rects[left_index], pane_rects[right_index])
                {
                    return false;
                }
            }
        }

        true
    }

    fn native_close_apply_candidate(
        panes: &mut [TerminalPane],
        removed: &TerminalPane,
        candidate: &NativeCloseCandidate,
    ) {
        let mut pane_rects = panes
            .iter()
            .map(Self::native_pane_rect_from_pane)
            .collect::<Vec<_>>();
        Self::native_close_expand_pane_rects(
            &mut pane_rects,
            &candidate.pane_indices,
            removed,
            candidate.direction,
        );

        for (pane, rect) in panes.iter_mut().zip(pane_rects) {
            pane.left = rect.left;
            pane.top = rect.top;
            pane.width = rect.width;
            pane.height = rect.height;
        }
    }

    pub(in super::super) fn native_close_expand_neighbors(
        panes: &mut [TerminalPane],
        removed: &TerminalPane,
    ) {
        if panes.is_empty() {
            return;
        }

        let removed_left = removed.left;
        let removed_top = removed.top;
        let removed_right = removed.left.saturating_add(removed.width);
        let removed_bottom = removed.top.saturating_add(removed.height);

        let mut left_candidates = Vec::<usize>::new();
        let mut right_candidates = Vec::<usize>::new();
        let mut top_candidates = Vec::<usize>::new();
        let mut bottom_candidates = Vec::<usize>::new();
        let mut left_intervals = Vec::<(u16, u16)>::new();
        let mut right_intervals = Vec::<(u16, u16)>::new();
        let mut top_intervals = Vec::<(u16, u16)>::new();
        let mut bottom_intervals = Vec::<(u16, u16)>::new();

        for (index, pane) in panes.iter().enumerate() {
            let pane_left = pane.left;
            let pane_top = pane.top;
            let pane_right = pane.left.saturating_add(pane.width);
            let pane_bottom = pane.top.saturating_add(pane.height);

            if pane_right == removed_left {
                let overlap =
                    Self::native_overlap_cells(pane_top, pane_bottom, removed_top, removed_bottom);
                if overlap > 0 {
                    left_candidates.push(index);
                    left_intervals
                        .push((pane_top.max(removed_top), pane_bottom.min(removed_bottom)));
                }
            }

            if pane_left == removed_right {
                let overlap =
                    Self::native_overlap_cells(pane_top, pane_bottom, removed_top, removed_bottom);
                if overlap > 0 {
                    right_candidates.push(index);
                    right_intervals
                        .push((pane_top.max(removed_top), pane_bottom.min(removed_bottom)));
                }
            }

            if pane_bottom == removed_top {
                let overlap =
                    Self::native_overlap_cells(pane_left, pane_right, removed_left, removed_right);
                if overlap > 0 {
                    top_candidates.push(index);
                    top_intervals
                        .push((pane_left.max(removed_left), pane_right.min(removed_right)));
                }
            }

            if pane_top == removed_bottom {
                let overlap =
                    Self::native_overlap_cells(pane_left, pane_right, removed_left, removed_right);
                if overlap > 0 {
                    bottom_candidates.push(index);
                    bottom_intervals
                        .push((pane_left.max(removed_left), pane_right.min(removed_right)));
                }
            }
        }

        let vertical_cover_target = removed_bottom.saturating_sub(removed_top);
        let horizontal_cover_target = removed_right.saturating_sub(removed_left);

        let mut candidates = [
            NativeCloseCandidate {
                direction: NativeCloseDirection::Left,
                pane_indices: left_candidates,
                coverage: Self::native_close_direction_coverage(
                    left_intervals,
                    removed_top,
                    removed_bottom,
                ),
                required_coverage: vertical_cover_target,
            },
            NativeCloseCandidate {
                direction: NativeCloseDirection::Right,
                pane_indices: right_candidates,
                coverage: Self::native_close_direction_coverage(
                    right_intervals,
                    removed_top,
                    removed_bottom,
                ),
                required_coverage: vertical_cover_target,
            },
            NativeCloseCandidate {
                direction: NativeCloseDirection::Top,
                pane_indices: top_candidates,
                coverage: Self::native_close_direction_coverage(
                    top_intervals,
                    removed_left,
                    removed_right,
                ),
                required_coverage: horizontal_cover_target,
            },
            NativeCloseCandidate {
                direction: NativeCloseDirection::Bottom,
                pane_indices: bottom_candidates,
                coverage: Self::native_close_direction_coverage(
                    bottom_intervals,
                    removed_left,
                    removed_right,
                ),
                required_coverage: horizontal_cover_target,
            },
        ];

        candidates.sort_by_key(|candidate| Reverse(candidate.coverage));

        if let Some(candidate) = candidates.iter().find(|candidate| {
            candidate.coverage >= candidate.required_coverage
                && Self::native_close_direction_preserves_layout(panes, removed, candidate)
        }) {
            Self::native_close_apply_candidate(panes, removed, candidate);
            return;
        }

        if let Some(candidate) = candidates.iter().find(|candidate| {
            !candidate.pane_indices.is_empty()
                && Self::native_close_direction_preserves_layout(panes, removed, candidate)
        }) {
            Self::native_close_apply_candidate(panes, removed, candidate);
            return;
        }

        log::warn!(
            "native pane close could not find a non-overlapping expansion target for removed pane {}",
            removed.id
        );
    }

    fn native_close_active_pane(&mut self, cx: &mut Context<Self>) -> bool {
        self.clear_native_zoom_snapshot_for_active_tab();
        let Some(tab) = self.session.tabs.get(self.session.active_tab) else {
            return false;
        };
        if tab.panes.len() <= 1 {
            return false;
        }
        let tab_id = tab.id;
        let Some(active_index) = tab.active_pane_index() else {
            return false;
        };
        let active_pane_id = tab.active_pane_id.clone();

        if self.ensure_native_layout_tree_for_tab_id(tab_id)
            && let Some(tree) = self.session.native_pane_layout_trees.remove(&tab_id)
        {
            let (next_root, next_focus_id, removed) =
                Self::native_remove_leaf_from_tree(tree.root, active_pane_id.as_str());
            if removed && let Some(next_root) = next_root {
                self.session
                    .native_pane_layout_trees
                    .insert(tab_id, NativePaneLayoutTree { root: next_root });
                if let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) {
                    tab.panes.retain(|pane| pane.id != active_pane_id);
                    let cols = tab
                        .panes
                        .iter()
                        .map(|pane| pane.left.saturating_add(pane.width))
                        .max()
                        .unwrap_or(1)
                        .max(1);
                    let rows = tab
                        .panes
                        .iter()
                        .map(|pane| pane.top.saturating_add(pane.height))
                        .max()
                        .unwrap_or(1)
                        .max(1);
                    let _ = tab;
                    self.apply_native_layout_tree_to_tab(tab_id, cols, rows);
                    if let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) {
                        tab.active_pane_id = next_focus_id
                            .or_else(|| tab.panes.first().map(|pane| pane.id.clone()))
                            .unwrap_or_default();
                        if tab.panes.len() == 1
                            && let Some(remaining_pane) = tab.panes.first_mut()
                        {
                            remaining_pane.pane_zoom_steps = 0;
                        }
                        tab.assert_active_pane_invariant();
                    }
                    self.clear_selection();
                    self.clear_hovered_link();
                    self.clear_terminal_scrollbar_marker_cache();
                    self.schedule_persist_native_workspace(cx);
                    cx.notify();
                    return true;
                }
            }
        }

        let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) else {
            return false;
        };
        let removed = tab.panes.remove(active_index);
        Self::native_close_expand_neighbors(&mut tab.panes, &removed);

        let next_index = active_index.min(tab.panes.len().saturating_sub(1));
        if let Some(next) = tab.panes.get(next_index) {
            tab.active_pane_id = next.id.clone();
        }
        if tab.panes.len() == 1
            && let Some(remaining_pane) = tab.panes.first_mut()
        {
            remaining_pane.pane_zoom_steps = 0;
        }
        tab.assert_active_pane_invariant();

        self.clear_selection();
        self.clear_hovered_link();
        self.clear_terminal_scrollbar_marker_cache();
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    fn native_focus_pane_direction(
        &mut self,
        direction: NativeFocusDirection,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.session.tabs.get(self.session.active_tab) else {
            return false;
        };
        let Some(active_index) = tab.active_pane_index() else {
            return false;
        };
        let Some(active) = tab.panes.get(active_index) else {
            return false;
        };

        let active_left = active.left;
        let active_top = active.top;
        let active_right = active.left.saturating_add(active.width);
        let active_bottom = active.top.saturating_add(active.height);

        let mut best: Option<(u16, Reverse<u16>, String)> = None;
        for pane in &tab.panes {
            if pane.id == active.id {
                continue;
            }

            let pane_left = pane.left;
            let pane_top = pane.top;
            let pane_right = pane.left.saturating_add(pane.width);
            let pane_bottom = pane.top.saturating_add(pane.height);

            let (distance, overlap) = match direction {
                NativeFocusDirection::Left => {
                    let overlap = Self::native_overlap_cells(
                        active_top,
                        active_bottom,
                        pane_top,
                        pane_bottom,
                    );
                    if overlap == 0 || pane_right > active_left {
                        continue;
                    }
                    (active_left.saturating_sub(pane_right), overlap)
                }
                NativeFocusDirection::Right => {
                    let overlap = Self::native_overlap_cells(
                        active_top,
                        active_bottom,
                        pane_top,
                        pane_bottom,
                    );
                    if overlap == 0 || pane_left < active_right {
                        continue;
                    }
                    (pane_left.saturating_sub(active_right), overlap)
                }
                NativeFocusDirection::Up => {
                    let overlap = Self::native_overlap_cells(
                        active_left,
                        active_right,
                        pane_left,
                        pane_right,
                    );
                    if overlap == 0 || pane_bottom > active_top {
                        continue;
                    }
                    (active_top.saturating_sub(pane_bottom), overlap)
                }
                NativeFocusDirection::Down => {
                    let overlap = Self::native_overlap_cells(
                        active_left,
                        active_right,
                        pane_left,
                        pane_right,
                    );
                    if overlap == 0 || pane_top < active_bottom {
                        continue;
                    }
                    (pane_top.saturating_sub(active_bottom), overlap)
                }
            };

            let candidate = (distance, Reverse(overlap), pane.id.clone());
            if best
                .as_ref()
                .is_none_or(|current| (candidate.0, candidate.1) < (current.0, current.1))
            {
                best = Some(candidate);
            }
        }

        let Some((_, _, pane_id)) = best else {
            return false;
        };
        self.native_focus_pane_target(pane_id.as_str(), cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_terminal() -> Terminal {
        Terminal::new_tmux(TerminalSize::default(), TerminalOptions::default())
    }

    fn test_pane(id: &str, left: u16, top: u16, width: u16, height: u16) -> TerminalPane {
        TerminalPane {
            id: id.to_string(),
            left,
            top,
            width,
            height,
            pane_zoom_steps: 0,
            degraded: false,
            tmux_mouse_mode: None,
            progress_state: ProgressState::default(),
            terminal: test_terminal(),
            render_cache: RefCell::new(TerminalPaneRenderCache::default()),
            last_alternate_screen: Cell::new(false),
            cached_element_ids: PaneCachedElementIds::new(id),
        }
    }

    #[test]
    fn adjacent_tab_index_moves_middle_tab_left_and_right() {
        assert_eq!(TerminalView::adjacent_tab_index(2, 5, false), Some(1));
        assert_eq!(TerminalView::adjacent_tab_index(2, 5, true), Some(3));
    }

    #[test]
    fn native_extra_tab_choices_control_the_new_tab_menu() {
        assert!(should_show_new_tab_menu(RuntimeKind::Native, true, false));
        assert!(!should_show_new_tab_menu(RuntimeKind::Native, false, false));
        assert!(should_show_new_tab_menu(RuntimeKind::Native, false, true));
        assert!(!should_show_new_tab_menu(RuntimeKind::Tmux, true, true));
    }

    #[test]
    fn selected_windows_shell_overrides_a_custom_shell_for_the_new_tab() {
        let base = TerminalRuntimeConfig {
            shell: Some(r"C:\tools\custom-shell.exe".to_string()),
            windows_shell: RuntimeWindowsShell::Cmd,
            term: "termy-test".to_string(),
            ..TerminalRuntimeConfig::default()
        };

        let runtime = runtime_config_for_windows_shell(&base, RuntimeWindowsShell::PowerShellCore);

        assert_eq!(runtime.shell, None);
        assert_eq!(runtime.windows_shell, RuntimeWindowsShell::PowerShellCore);
        assert_eq!(runtime.term, "termy-test");
    }

    #[test]
    fn adjacent_pane_index_wraps_for_next_and_previous() {
        assert_eq!(TerminalView::adjacent_pane_index(2, 4, 1), Some(3));
        assert_eq!(TerminalView::adjacent_pane_index(3, 4, 1), Some(0));
        assert_eq!(TerminalView::adjacent_pane_index(0, 4, -1), Some(3));
        assert_eq!(TerminalView::adjacent_pane_index(2, 4, -1), Some(1));
    }

    #[test]
    fn adjacent_pane_index_is_none_for_invalid_or_no_movement() {
        assert_eq!(TerminalView::adjacent_pane_index(0, 0, 1), None);
        assert_eq!(TerminalView::adjacent_pane_index(0, 1, 1), None);
        assert_eq!(TerminalView::adjacent_pane_index(2, 2, 1), None);
        assert_eq!(TerminalView::adjacent_pane_index(0, 2, 0), None);
    }

    #[test]
    fn adjacent_tab_index_is_none_for_edges() {
        assert_eq!(TerminalView::adjacent_tab_index(0, 5, false), None);
        assert_eq!(TerminalView::adjacent_tab_index(4, 5, true), None);
    }

    #[test]
    fn adjacent_tab_index_is_none_for_invalid_or_singleton_state() {
        assert_eq!(TerminalView::adjacent_tab_index(0, 0, false), None);
        assert_eq!(TerminalView::adjacent_tab_index(0, 1, true), None);
        assert_eq!(TerminalView::adjacent_tab_index(5, 3, true), None);
    }

    #[test]
    fn cycled_tab_index_wraps_from_last_to_first_tab() {
        assert_eq!(TerminalView::cycled_tab_index(0, 4), Some(1));
        assert_eq!(TerminalView::cycled_tab_index(2, 4), Some(3));
        assert_eq!(TerminalView::cycled_tab_index(3, 4), Some(0));
    }

    #[test]
    fn cycled_tab_index_is_none_for_invalid_or_singleton_state() {
        assert_eq!(TerminalView::cycled_tab_index(0, 0), None);
        assert_eq!(TerminalView::cycled_tab_index(0, 1), None);
        assert_eq!(TerminalView::cycled_tab_index(3, 3), None);
    }

    #[test]
    fn remap_index_after_move_handles_move_to_right() {
        assert_eq!(TerminalView::remap_index_after_move(1, 1, 3), 3);
        assert_eq!(TerminalView::remap_index_after_move(2, 1, 3), 1);
        assert_eq!(TerminalView::remap_index_after_move(3, 1, 3), 2);
        assert_eq!(TerminalView::remap_index_after_move(0, 1, 3), 0);
    }

    #[test]
    fn remap_index_after_move_handles_move_to_left() {
        assert_eq!(TerminalView::remap_index_after_move(3, 3, 1), 1);
        assert_eq!(TerminalView::remap_index_after_move(1, 3, 1), 2);
        assert_eq!(TerminalView::remap_index_after_move(2, 3, 1), 3);
        assert_eq!(TerminalView::remap_index_after_move(4, 3, 1), 4);
    }

    #[test]
    fn remap_index_after_move_keeps_moved_tab_active() {
        assert_eq!(TerminalView::remap_index_after_move(2, 2, 1), 1);
        assert_eq!(TerminalView::remap_index_after_move(2, 2, 3), 3);
    }

    #[test]
    fn close_pane_or_tab_target_prefers_pane_for_tmux_multi_pane_tabs() {
        assert_eq!(
            TerminalView::close_pane_or_tab_target(RuntimeKind::Tmux, 2),
            ClosePaneOrTabTarget::ClosePane
        );
    }

    #[test]
    fn close_pane_or_tab_target_falls_back_to_tab_when_last_pane() {
        assert_eq!(
            TerminalView::close_pane_or_tab_target(RuntimeKind::Tmux, 1),
            ClosePaneOrTabTarget::CloseTab
        );
        assert_eq!(
            TerminalView::close_pane_or_tab_target(RuntimeKind::Tmux, 0),
            ClosePaneOrTabTarget::CloseTab
        );
    }

    #[test]
    fn close_pane_or_tab_target_prefers_pane_when_multiple_exist() {
        assert_eq!(
            TerminalView::close_pane_or_tab_target(RuntimeKind::Native, 3),
            ClosePaneOrTabTarget::ClosePane
        );
    }

    #[test]
    fn native_close_expand_neighbors_avoids_expanding_into_overlapping_layout() {
        let removed = test_pane("%native-1", 0, 0, 60, 20);
        let mut panes = vec![
            test_pane("%native-2", 60, 0, 60, 20),
            test_pane("%native-3", 0, 20, 120, 20),
        ];

        TerminalView::native_close_expand_neighbors(&mut panes, &removed);

        assert_eq!(panes[0].left, 0);
        assert_eq!(panes[0].top, 0);
        assert_eq!(panes[0].width, 120);
        assert_eq!(panes[0].height, 20);
        assert_eq!(panes[1].left, 0);
        assert_eq!(panes[1].top, 20);
        assert_eq!(panes[1].width, 120);
        assert_eq!(panes[1].height, 20);
        assert!(!TerminalView::native_pane_rects_overlap(
            TerminalView::native_pane_rect_from_pane(&panes[0]),
            TerminalView::native_pane_rect_from_pane(&panes[1]),
        ));
    }
}
