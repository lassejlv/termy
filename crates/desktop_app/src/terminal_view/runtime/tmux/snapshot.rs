use super::*;
use crate::terminal_ui::{TmuxClient, TmuxPaneState, TmuxSnapshot};

fn window_order_index(window_order: &[&str], target_window_id: Option<&str>) -> Option<usize> {
    target_window_id.and_then(|target| {
        window_order
            .iter()
            .position(|window_id| *window_id == target)
    })
}

fn snapshot_preferred_cwd(snapshot: &TmuxSnapshot) -> Option<String> {
    snapshot
        .windows
        .iter()
        .find(|window| window.is_active)
        .or_else(|| snapshot.windows.first())
        .and_then(|window| {
            window
                .active_pane_id
                .as_deref()
                .and_then(|pane_id| window.panes.iter().find(|pane| pane.id == pane_id))
                .or_else(|| window.panes.first())
        })
        .map(|pane| pane.current_path.trim())
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
}

impl TmuxRuntime {
    fn merged_preferred_cwd(existing: Option<&str>, snapshot: &TmuxSnapshot) -> Option<String> {
        snapshot_preferred_cwd(snapshot).or_else(|| existing.map(ToOwned::to_owned))
    }

    fn update_preferred_cwd_from_snapshot(&mut self, snapshot: &TmuxSnapshot) {
        self.preferred_cwd = Self::merged_preferred_cwd(self.preferred_cwd.as_deref(), snapshot);
    }
}

impl TerminalView {
    pub(in crate::terminal_view) fn tmux_pane_working_dir(
        snapshot: &TmuxSnapshot,
        pane_id: &str,
    ) -> Option<String> {
        snapshot
            .windows
            .iter()
            .flat_map(|window| &window.panes)
            .find(|pane| pane.id == pane_id)
            .and_then(|pane| normalize_working_directory_candidate(Some(&pane.current_path)))
    }

    fn hydration_capture_scrollback_history(
        active_scrollback_history: usize,
        inactive_tab_scrollback: Option<usize>,
    ) -> usize {
        inactive_tab_scrollback.map_or(active_scrollback_history, |inactive_history| {
            inactive_history.max(active_scrollback_history)
        })
    }

    fn hydration_capture_row_budget(scrollback_history: usize, pane_height: u16) -> usize {
        scrollback_history
            .saturating_add(usize::from(pane_height.max(1)))
            .max(1)
    }

    fn terminal_size_for_pane_state(
        pane: &TmuxPaneState,
        cell_size: Option<Size<Pixels>>,
    ) -> TerminalSize {
        let default_size = TerminalSize::default();
        let (cell_width, cell_height) = if let Some(cell_size) = cell_size {
            (cell_size.width.into(), cell_size.height.into())
        } else {
            (default_size.cell_width, default_size.cell_height)
        };

        TerminalSize {
            cols: pane.width.max(1),
            rows: pane.height.max(1),
            cell_width,
            cell_height,
        }
    }

    fn hydrate_pane_terminal(
        tmux_client: &TmuxClient,
        pane: &TmuxPaneState,
        options: TerminalOptions,
        cell_size: Option<Size<Pixels>>,
    ) -> (Terminal, Option<String>) {
        let terminal =
            Terminal::new_tmux(Self::terminal_size_for_pane_state(pane, cell_size), options);

        // Always rebuild panes from full tmux history and let terminal content
        // define cursor state. Snapshot cursor coordinates can drift relative to
        // captured rows across attach/switch timing, so we do not inject cursor
        // position escapes during hydration.
        // Bound capture to local retention + current viewport so hydration keeps
        // user-visible context without paying unbounded tmux history costs.
        let capture_rows =
            Self::hydration_capture_row_budget(options.scrollback_history, pane.height);
        let capture = tmux_client.capture_pane(&pane.id, capture_rows);

        match capture {
            Ok(capture) => {
                terminal.feed_output(&capture);
                (terminal, None)
            }
            Err(error) => {
                // Snapshot-driven pane rehydrate must stay non-fatal: create an empty
                // terminal buffer, mark the pane degraded, and surface one warning later.
                (terminal, Some(error.to_string()))
            }
        }
    }

    pub(super) fn apply_tmux_snapshot_rehydrate(&mut self, snapshot: TmuxSnapshot) {
        self.apply_tmux_snapshot_inner(snapshot, false);
    }

    fn apply_tmux_snapshot_inner(
        &mut self,
        snapshot: TmuxSnapshot,
        reuse_existing_terminals: bool,
    ) {
        if self.runtime_uses_tmux() {
            self.tmux_runtime_mut()
                .update_preferred_cwd_from_snapshot(&snapshot);
        }
        let snapshot_pane_ids = snapshot
            .windows
            .iter()
            .flat_map(|window| window.panes.iter().map(|pane| pane.id.as_str()))
            .collect::<std::collections::HashSet<_>>();
        let removed_pane_ids = self
            .session
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter().map(|pane| pane.id.clone()))
            .filter(|pane_id| !snapshot_pane_ids.contains(pane_id.as_str()))
            .collect::<Vec<_>>();
        let _ = self.release_forwarded_mouse_presses_for_panes(&removed_pane_ids);

        let previous_active_window_id = self
            .session
            .tabs
            .get(self.session.active_tab)
            .map(|tab| tab.window_id.clone());
        let previous_renaming_window_id = self.renaming_tab.and_then(|index| {
            self.session
                .tabs
                .get(index)
                .map(|tab| tab.window_id.clone())
        });
        let previous_ids = self
            .session
            .tabs
            .iter()
            .map(|tab| (tab.window_id.clone(), tab.id))
            .collect::<std::collections::HashMap<_, _>>();
        let previous_pins = self
            .session
            .tabs
            .iter()
            .map(|tab| (tab.window_id.clone(), tab.pinned))
            .collect::<std::collections::HashMap<_, _>>();
        let mut existing_terminals = std::collections::HashMap::<String, Terminal>::new();
        let old_tabs = std::mem::take(&mut self.session.tabs);
        if reuse_existing_terminals {
            for mut tab in old_tabs {
                for pane in tab.panes.drain(..) {
                    existing_terminals.insert(pane.id, pane.terminal);
                }
            }
        }

        let mut new_tabs = Vec::new();
        let mut hydration_failures = Vec::<String>::new();
        let hydration_scrollback_history = Self::hydration_capture_scrollback_history(
            self.terminal_runtime.scrollback_history,
            self.inactive_tab_scrollback,
        );
        let hydration_options = self
            .terminal_runtime
            .term_options()
            .with_scrollback_history(hydration_scrollback_history);
        for window in &snapshot.windows {
            let mut panes = Vec::new();
            for pane_state in &window.panes {
                let (terminal, degraded, hydration_error) =
                    if let Some(existing) = existing_terminals.remove(&pane_state.id) {
                        (existing, false, None)
                    } else {
                        let (terminal, hydration_error) = Self::hydrate_pane_terminal(
                            &self.tmux_runtime().client,
                            pane_state,
                            hydration_options,
                            self.cached_cell_size_for_font_size(self.font_size),
                        );
                        (terminal, hydration_error.is_some(), hydration_error)
                    };

                if let Some(hydration_error) = hydration_error {
                    hydration_failures.push(format!("{} ({hydration_error})", pane_state.id));
                }

                let next_size = Self::terminal_size_for_pane_state(
                    pane_state,
                    self.cached_cell_size_for_font_size(self.font_size),
                );
                let current_size = terminal.size();
                if current_size.cols != next_size.cols
                    || current_size.rows != next_size.rows
                    || current_size.cell_width != next_size.cell_width
                    || current_size.cell_height != next_size.cell_height
                {
                    terminal.resize(next_size);
                }
                panes.push(TerminalPane::from_tmux_state(
                    pane_state, terminal, degraded,
                ));
            }

            let tab_id = previous_ids
                .get(&window.id)
                .copied()
                .unwrap_or_else(|| self.allocate_tab_id());
            let active_pane_state = window
                .active_pane_id
                .as_deref()
                .and_then(|pane_id| window.panes.iter().find(|pane| pane.id == pane_id))
                .or_else(|| window.panes.first());
            let manual_title = (!window.automatic_rename)
                .then_some(window.name.trim())
                .and_then(|name| (!name.is_empty()).then(|| Self::truncate_tab_title(name)));
            let shell_title = active_pane_state
                .and_then(|pane| Self::derive_tmux_shell_title(&self.tab_title, pane));
            let running_process = active_pane_state
                .is_some_and(|pane| !Self::is_shell_command(pane.current_command.as_str()));
            let current_command = active_pane_state
                .map(|pane| pane.current_command.trim())
                .filter(|command| !command.is_empty() && !Self::is_shell_command(command))
                .map(ToOwned::to_owned);

            let mut tab = TerminalTab::from_tmux_window(tab_id, window, panes);
            tab.pinned = previous_pins.get(&window.id).copied().unwrap_or(false);
            tab.manual_title = manual_title;
            tab.shell_title = shell_title;
            tab.current_command = current_command;
            tab.running_process = running_process;
            new_tabs.push(tab);
        }

        new_tabs.sort_by_key(|tab| tab.window_index);
        self.session.tabs = new_tabs;
        let tab_window_order = self
            .session
            .tabs
            .iter()
            .map(|tab| tab.window_id.as_str())
            .collect::<Vec<_>>();

        let mut next_id = 1;
        for tab in &self.session.tabs {
            next_id = next_id.max(tab.id.saturating_add(1));
        }
        self.session.next_tab_id = next_id;

        let active_index_by_window = snapshot
            .windows
            .iter()
            .find(|window| window.is_active)
            .map(|window| window.id.as_str())
            .and_then(|window_id| window_order_index(&tab_window_order, Some(window_id)));
        let previous_index =
            window_order_index(&tab_window_order, previous_active_window_id.as_deref());
        self.session.active_tab = active_index_by_window
            .or(previous_index)
            .unwrap_or(0)
            .min(self.session.tabs.len().saturating_sub(1));

        if self.session.tabs.is_empty() {
            self.session.active_tab = 0;
        }
        // Rename state tracks the original window identity, not stale index
        // positions that can drift when tmux reorders/closes windows.
        self.renaming_tab =
            window_order_index(&tab_window_order, previous_renaming_window_id.as_deref());
        for index in 0..self.session.tabs.len() {
            self.refresh_tab_title(index);
        }
        let inactive_history = self
            .inactive_tab_scrollback
            .unwrap_or(self.terminal_runtime.scrollback_history);
        let active_options = self.terminal_runtime.term_options();
        let inactive_options = (inactive_history != active_options.scrollback_history)
            .then(|| active_options.with_scrollback_history(inactive_history));
        for (tab_index, tab) in self.session.tabs.iter().enumerate() {
            let options = if tab_index == self.session.active_tab {
                active_options
            } else {
                inactive_options.unwrap_or(active_options)
            };
            for pane in &tab.panes {
                pane.terminal().set_term_options(options);
            }
        }
        self.mark_tab_strip_layout_dirty();
        self.sync_tab_strip_for_active_tab();

        if let Some(message) = tmux_hydration_warning_message(&hydration_failures) {
            crate::ui::toast::warning(message);
        }
    }

    pub(in crate::terminal_view) fn apply_tmux_snapshot(&mut self, snapshot: TmuxSnapshot) {
        self.apply_tmux_snapshot_inner(snapshot, true);
    }

    pub(in crate::terminal_view) fn refresh_tmux_snapshot(&mut self) -> bool {
        match self.tmux_runtime().client.refresh_snapshot() {
            Ok(snapshot) => {
                self.apply_tmux_snapshot(snapshot);
                true
            }
            Err(error) => {
                crate::ui::toast::error(format!("tmux sync failed: {error}"));
                false
            }
        }
    }

    pub(super) fn snapshot_matches_client_size(
        snapshot: &TmuxSnapshot,
        cols: u16,
        rows: u16,
    ) -> bool {
        let expected_cols = u32::from(cols.max(1));
        let expected_rows = u32::from(rows.max(1));
        snapshot
            .windows
            .iter()
            .filter(|window| !window.panes.is_empty())
            .all(|window| {
                let max_right = window
                    .panes
                    .iter()
                    .map(|pane| u32::from(pane.left).saturating_add(u32::from(pane.width)))
                    .max()
                    .unwrap_or(0);
                let max_bottom = window
                    .panes
                    .iter()
                    .map(|pane| u32::from(pane.top).saturating_add(u32::from(pane.height)))
                    .max()
                    .unwrap_or(0);
                let min_left = window
                    .panes
                    .iter()
                    .map(|pane| u32::from(pane.left))
                    .min()
                    .unwrap_or(0);
                let min_top = window
                    .panes
                    .iter()
                    .map(|pane| u32::from(pane.top))
                    .min()
                    .unwrap_or(0);
                max_right == expected_cols
                    && max_bottom == expected_rows
                    && min_left == 0
                    && min_top == 0
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_state(width: u16, height: u16) -> TmuxPaneState {
        TmuxPaneState {
            id: "%1".to_string(),
            window_id: "@1".to_string(),
            session_id: "$1".to_string(),
            is_active: true,
            left: 0,
            top: 0,
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            mouse_mode: TmuxPaneMouseMode::default(),
            current_path: String::new(),
            current_command: String::new(),
        }
    }

    #[test]
    fn terminal_size_for_pane_state_clamps_zero_dimensions() {
        let pane = pane_state(0, 0);
        let size = TerminalView::terminal_size_for_pane_state(&pane, None);
        assert_eq!(size.cols, 1);
        assert_eq!(size.rows, 1);
    }

    #[test]
    fn hydration_capture_row_budget_includes_scrollback_and_viewport_rows() {
        assert_eq!(TerminalView::hydration_capture_row_budget(2000, 60), 2060);
    }

    #[test]
    fn hydration_capture_row_budget_clamps_zero_height_to_one_row() {
        assert_eq!(TerminalView::hydration_capture_row_budget(0, 0), 1);
    }

    #[test]
    fn hydration_capture_row_budget_saturates_on_large_inputs() {
        assert_eq!(
            TerminalView::hydration_capture_row_budget(usize::MAX, u16::MAX),
            usize::MAX
        );
    }

    #[test]
    fn hydration_capture_scrollback_history_uses_active_when_inactive_not_set() {
        assert_eq!(
            TerminalView::hydration_capture_scrollback_history(2_000, None),
            2_000
        );
    }

    #[test]
    fn hydration_capture_scrollback_history_uses_max_of_active_and_inactive() {
        assert_eq!(
            TerminalView::hydration_capture_scrollback_history(2_000, Some(4_000)),
            4_000
        );
        assert_eq!(
            TerminalView::hydration_capture_scrollback_history(2_000, Some(1_000)),
            2_000
        );
    }

    #[test]
    fn window_order_index_maps_stable_window_identity_after_reorder() {
        let order = vec!["@2", "@1", "@3"];
        assert_eq!(window_order_index(&order, Some("@1")), Some(1));
        assert_eq!(window_order_index(&order, Some("@2")), Some(0));
        assert_eq!(window_order_index(&order, Some("@missing")), None);
    }

    fn pane_state_with_path(id: &str, path: &str, active: bool) -> TmuxPaneState {
        TmuxPaneState {
            id: id.to_string(),
            window_id: "@1".to_string(),
            session_id: "$1".to_string(),
            is_active: active,
            left: 0,
            top: 0,
            width: 80,
            height: 24,
            cursor_x: 0,
            cursor_y: 0,
            mouse_mode: TmuxPaneMouseMode::default(),
            current_path: path.to_string(),
            current_command: String::new(),
        }
    }

    #[test]
    fn snapshot_preferred_cwd_uses_active_pane_path() {
        let snapshot = TmuxSnapshot {
            session_name: "one".to_string(),
            session_id: Some("$1".to_string()),
            windows: vec![TmuxWindowState {
                id: "@1".to_string(),
                name: "one".to_string(),
                index: 0,
                layout: "layout".to_string(),
                is_active: true,
                active_pane_id: Some("%2".to_string()),
                automatic_rename: true,
                panes: vec![
                    pane_state_with_path("%1", "/inactive", false),
                    pane_state_with_path("%2", "/active", true),
                ],
            }],
        };

        assert_eq!(
            snapshot_preferred_cwd(&snapshot).as_deref(),
            Some("/active")
        );
        // Target the UI-selected pane, not whichever pane a snapshot marks active.
        // Remote tmux paths need not exist on the client filesystem.
        assert_eq!(
            TerminalView::tmux_pane_working_dir(&snapshot, "%1").as_deref(),
            Some("/inactive")
        );
        assert!(TerminalView::tmux_pane_working_dir(&snapshot, "%missing").is_none());
    }

    #[test]
    fn snapshot_preferred_cwd_skips_empty_paths() {
        let snapshot = TmuxSnapshot {
            session_name: "one".to_string(),
            session_id: Some("$1".to_string()),
            windows: vec![TmuxWindowState {
                id: "@1".to_string(),
                name: "one".to_string(),
                index: 0,
                layout: "layout".to_string(),
                is_active: true,
                active_pane_id: Some("%1".to_string()),
                automatic_rename: true,
                panes: vec![pane_state_with_path("%1", "   ", true)],
            }],
        };

        assert!(snapshot_preferred_cwd(&snapshot).is_none());
    }

    #[test]
    fn merged_preferred_cwd_preserves_existing_value_for_empty_paths() {
        let snapshot = TmuxSnapshot {
            session_name: "one".to_string(),
            session_id: Some("$1".to_string()),
            windows: vec![TmuxWindowState {
                id: "@1".to_string(),
                name: "one".to_string(),
                index: 0,
                layout: "layout".to_string(),
                is_active: true,
                active_pane_id: Some("%1".to_string()),
                automatic_rename: true,
                panes: vec![pane_state_with_path("%1", "   ", true)],
            }],
        };

        assert_eq!(
            TmuxRuntime::merged_preferred_cwd(Some("/existing/path"), &snapshot).as_deref(),
            Some("/existing/path")
        );
    }
}
