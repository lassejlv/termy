use super::*;

/// A workspace groups tabs under one sidebar entry. The active workspace's
/// tabs live in `TerminalView::session`; only inactive workspaces keep their
/// tabs stashed here. Switching workspaces swaps the whole tab vec so the
/// index-based tab machinery (strip, drag, persistence) never has to filter.
pub(crate) struct WorkspaceEntry {
    pub(crate) id: u64,
    pub(crate) name: String,
    /// `true` once the user renamed the workspace. While `false` the sidebar
    /// shows an auto-title derived from the workspace's active tab instead of
    /// `name`, which then only serves as the fallback label.
    pub(crate) custom_named: bool,
    pub(crate) pinned: bool,
    pub(crate) tabs: Vec<TerminalTab>,
    pub(crate) active_tab: usize,
    /// Persisted tabs that have not started their PTYs yet. Inactive restored
    /// workspaces stay cold until the user switches to them.
    pub(crate) pending_restore: Option<crate::workspace_store::StoredWorkspace>,
    /// Something happened in this workspace while it was stashed (bell or a
    /// command finished). Cleared when the workspace is activated; never set
    /// on the active workspace. Not persisted.
    pub(crate) attention: bool,
}

/// Sidebar status for one workspace, most urgent first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceStatus {
    Idle,
    Busy,
    Attention,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkspaceDeleteBlocker {
    Missing,
    LastWorkspace,
    PinnedWorkspace,
    PinnedTab,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WorkspaceSidebarResizeDragState {
    start_window_x: f32,
    start_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceDragState {
    source_index: usize,
    drop_slot: Option<usize>,
}

impl WorkspaceEntry {
    pub(crate) fn new(id: u64) -> Self {
        Self {
            id,
            name: format!("Workspace {id}"),
            custom_named: false,
            pinned: false,
            tabs: Vec::new(),
            active_tab: 0,
            pending_restore: None,
            attention: false,
        }
    }

    /// Whether `name` looks like an auto-generated `Workspace {id}` label.
    /// Used when restoring sessions persisted before the `custom_named` flag
    /// existed, so old default names keep auto-titling.
    pub(crate) fn is_default_workspace_name(name: &str) -> bool {
        name.strip_prefix("Workspace ")
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
    }
}

impl TerminalView {
    pub(crate) fn clamp_workspace_sidebar_width(width: f32) -> f32 {
        if width.is_finite() {
            width.clamp(
                termy_config_core::MIN_SIDEBAR_WIDTH,
                termy_config_core::MAX_SIDEBAR_WIDTH,
            )
        } else {
            termy_config_core::DEFAULT_SIDEBAR_WIDTH
        }
    }

    pub(crate) fn workspace_sidebar_visible(&self) -> bool {
        self.workspace_sidebar_enabled
            && !self.workspace_sidebar_collapsed
            && !self.simple_mode
            && self.effective_tab_bar_visibility() != TabBarVisibility::ForceHidden
    }

    pub(crate) fn workspace_sidebar_overlay_visible(&self) -> bool {
        self.workspace_sidebar_enabled
            && self.workspace_sidebar_collapsed
            && self.workspace_sidebar_peek_visible
            && !self.simple_mode
            && self.effective_tab_bar_visibility() != TabBarVisibility::ForceHidden
    }

    pub(crate) fn workspace_sidebar_edge_peek_enabled(&self) -> bool {
        self.workspace_sidebar_enabled
            && self.workspace_sidebar_collapsed
            && !self.simple_mode
            && self.effective_tab_bar_visibility() != TabBarVisibility::ForceHidden
    }

    /// Width reserved on the left for the workspace sidebar. Feeds the
    /// terminal grid sizer and window-to-surface mouse coordinate mapping.
    pub(crate) fn workspace_sidebar_width(&self) -> f32 {
        if self.workspace_sidebar_visible() {
            self.workspace_sidebar_width
        } else {
            0.0
        }
    }

    pub(crate) fn set_workspace_sidebar_peek_visible(
        &mut self,
        visible: bool,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_sidebar_peek_visible == visible {
            return;
        }
        self.workspace_sidebar_peek_visible = visible;
        cx.notify();
    }

    pub(crate) fn toggle_workspace_sidebar_collapsed(&mut self, cx: &mut Context<Self>) {
        let Some(collapsed) = Self::next_workspace_sidebar_collapsed(
            self.workspace_sidebar_enabled,
            self.workspace_sidebar_collapsed,
        ) else {
            return;
        };

        self.workspace_sidebar_collapsed = collapsed;
        if !self.workspace_sidebar_collapsed {
            self.workspace_sidebar_peek_visible = false;
        }
        self.mark_tab_strip_layout_dirty();
        cx.notify();
    }

    fn next_workspace_sidebar_collapsed(enabled: bool, collapsed: bool) -> Option<bool> {
        enabled.then_some(!collapsed)
    }

    pub(crate) fn begin_workspace_sidebar_resize_drag(&mut self, window_x: f32) {
        self.workspace_sidebar_resize_drag = Some(WorkspaceSidebarResizeDragState {
            start_window_x: window_x,
            start_width: self.workspace_sidebar_width,
        });
    }

    pub(crate) fn workspace_sidebar_resize_drag_active(&self) -> bool {
        self.workspace_sidebar_resize_drag.is_some()
    }

    pub(crate) fn update_workspace_sidebar_resize_drag(&mut self, window_x: f32) -> bool {
        let Some(drag) = self.workspace_sidebar_resize_drag else {
            return false;
        };
        let next_width = Self::clamp_workspace_sidebar_width(
            drag.start_width + (window_x - drag.start_window_x),
        );
        if (next_width - self.workspace_sidebar_width).abs() < f32::EPSILON {
            return false;
        }
        self.workspace_sidebar_width = next_width;
        self.mark_tab_strip_layout_dirty();
        true
    }

    pub(crate) fn finish_workspace_sidebar_resize_drag(&mut self) -> bool {
        let Some(drag) = self.workspace_sidebar_resize_drag.take() else {
            return false;
        };
        if (self.workspace_sidebar_width - drag.start_width).abs() >= 1.0
            && let Err(error) = crate::config::set_root_setting(
                termy_config_core::RootSettingId::SidebarWidth,
                &format!("{:.0}", self.workspace_sidebar_width),
            )
        {
            log::error!("Failed to persist workspace sidebar width: {error}");
        }
        self.mark_tab_strip_layout_dirty();
        true
    }

    pub(crate) fn sync_workspace_sidebar_width_from_config(
        &mut self,
        configured_width: f32,
    ) -> bool {
        if self.workspace_sidebar_resize_drag.is_some() {
            return false;
        }
        let next_width = Self::clamp_workspace_sidebar_width(configured_width);
        if (next_width - self.workspace_sidebar_width).abs() < f32::EPSILON {
            return false;
        }
        self.workspace_sidebar_width = next_width;
        true
    }

    pub(crate) fn has_other_workspaces(&self) -> bool {
        self.session.workspaces.len() > 1
    }

    pub(super) fn workspace_delete_blocker(
        &self,
        workspace_id: u64,
    ) -> Option<WorkspaceDeleteBlocker> {
        let Some(index) = self
            .session
            .workspaces
            .iter()
            .position(|entry| entry.id == workspace_id)
        else {
            return Some(WorkspaceDeleteBlocker::Missing);
        };
        if self.session.workspaces.len() <= 1 {
            return Some(WorkspaceDeleteBlocker::LastWorkspace);
        }

        let entry = &self.session.workspaces[index];
        if entry.pinned {
            return Some(WorkspaceDeleteBlocker::PinnedWorkspace);
        }
        let live_tabs = if index == self.session.active_workspace {
            self.session.tabs.as_slice()
        } else {
            entry.tabs.as_slice()
        };
        let has_pinned_tab = live_tabs.iter().any(|tab| tab.pinned)
            || entry
                .pending_restore
                .as_ref()
                .is_some_and(|stored| stored.tabs.iter().any(|tab| tab.pinned));
        has_pinned_tab.then_some(WorkspaceDeleteBlocker::PinnedTab)
    }

    /// Sidebar status for the workspace at `index`. The active workspace only
    /// reports Busy/Idle: the user is already looking at it, so its attention
    /// flag stays cleared.
    pub(crate) fn workspace_status(&self, index: usize) -> WorkspaceStatus {
        let Some(entry) = self.session.workspaces.get(index) else {
            return WorkspaceStatus::Idle;
        };
        let tabs = if index == self.session.active_workspace {
            self.session.tabs.as_slice()
        } else {
            if entry.attention {
                return WorkspaceStatus::Attention;
            }
            entry.tabs.as_slice()
        };
        if tabs.iter().any(Self::tab_is_busy) {
            WorkspaceStatus::Busy
        } else {
            WorkspaceStatus::Idle
        }
    }

    /// Sidebar label for the workspace at `index`. User-renamed workspaces
    /// keep their name; otherwise the label follows the workspace's active
    /// tab title, falling back to the default `Workspace {id}` name. Titles
    /// on stashed tabs are frozen at stash time, so inactive workspaces show
    /// the title from when they were last active.
    pub(crate) fn workspace_display_name(&self, index: usize) -> String {
        let Some(entry) = self.session.workspaces.get(index) else {
            return String::new();
        };
        if entry.custom_named {
            return entry.name.clone();
        }
        let active_tab_title = if index == self.session.active_workspace {
            self.session.tabs.get(self.session.active_tab)
        } else {
            entry.tabs.get(entry.active_tab)
        }
        .map(|tab| tab.title.trim())
        .filter(|title| !title.is_empty());
        match active_tab_title {
            Some(title) => Self::truncate_tab_title(title),
            None => entry.name.clone(),
        }
    }

    pub(crate) fn begin_workspace_drag(&mut self, index: usize) {
        if index >= self.session.workspaces.len() || self.renaming_workspace.is_some() {
            return;
        }
        self.workspace_drag = Some(WorkspaceDragState {
            source_index: index,
            drop_slot: None,
        });
    }

    pub(crate) fn update_workspace_drag_over(
        &mut self,
        hover_index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.workspace_drag else {
            return;
        };
        if hover_index >= self.session.workspaces.len()
            || drag.source_index >= self.session.workspaces.len()
        {
            return;
        }

        let raw_slot = if hover_index > drag.source_index {
            hover_index.saturating_add(1)
        } else {
            hover_index
        };
        let clamped_slot = self.clamp_workspace_drop_slot_to_pin_group(drag.source_index, raw_slot);
        let next_drop_slot = Self::normalized_workspace_drop_slot(drag.source_index, clamped_slot);
        let Some(drag) = self.workspace_drag.as_mut() else {
            return;
        };
        if drag.drop_slot == next_drop_slot {
            return;
        }

        drag.drop_slot = next_drop_slot;
        cx.notify();
    }

    pub(crate) fn finish_workspace_drag(&mut self) -> bool {
        self.workspace_drag.take().is_some()
    }

    pub(crate) fn workspace_drop_marker_side(
        &self,
        index: usize,
    ) -> Option<crate::terminal_view::tab_strip::state::TabDropMarkerSide> {
        if index >= self.session.workspaces.len() {
            return None;
        }
        let drop_slot = self.workspace_drag.and_then(|drag| drag.drop_slot)?;
        if drop_slot == index {
            Some(crate::terminal_view::tab_strip::state::TabDropMarkerSide::Leading)
        } else if drop_slot == index.saturating_add(1) {
            Some(crate::terminal_view::tab_strip::state::TabDropMarkerSide::Trailing)
        } else {
            None
        }
    }

    pub(crate) fn commit_workspace_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(WorkspaceDragState {
            source_index,
            drop_slot: Some(drop_slot),
        }) = self.workspace_drag.take()
        else {
            return false;
        };
        if source_index >= self.session.workspaces.len() {
            return false;
        }
        let target_index =
            Self::workspace_reorder_target_index_for_drop_slot(source_index, drop_slot);
        if target_index >= self.session.workspaces.len() || source_index == target_index {
            return false;
        }

        let active_id = self
            .session
            .workspaces
            .get(self.session.active_workspace)
            .map(|entry| entry.id);
        let entry = self.session.workspaces.remove(source_index);
        self.session.workspaces.insert(target_index, entry);
        if let Some(active_id) = active_id
            && let Some(next_active) = self
                .session
                .workspaces
                .iter()
                .position(|entry| entry.id == active_id)
        {
            self.session.active_workspace = next_active;
        }
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    fn normalized_workspace_drop_slot(source_index: usize, raw_slot: usize) -> Option<usize> {
        if raw_slot == source_index || raw_slot == source_index.saturating_add(1) {
            return None;
        }
        Some(raw_slot)
    }

    fn workspace_reorder_target_index_for_drop_slot(
        source_index: usize,
        drop_slot: usize,
    ) -> usize {
        if drop_slot > source_index {
            drop_slot - 1
        } else {
            drop_slot
        }
    }

    fn workspace_pin_group_bounds(&self, source_index: usize) -> (usize, usize) {
        let Some(source) = self.session.workspaces.get(source_index) else {
            return (0, self.session.workspaces.len());
        };
        let mut start = source_index;
        while start > 0 && self.session.workspaces[start - 1].pinned == source.pinned {
            start -= 1;
        }
        let mut end = source_index + 1;
        while end < self.session.workspaces.len()
            && self.session.workspaces[end].pinned == source.pinned
        {
            end += 1;
        }
        (start, end)
    }

    fn clamp_workspace_drop_slot_to_pin_group(
        &self,
        source_index: usize,
        raw_slot: usize,
    ) -> usize {
        let (start, end) = self.workspace_pin_group_bounds(source_index);
        raw_slot.clamp(start, end)
    }

    fn reorder_workspaces_for_pins(&mut self) {
        if self.session.workspaces.len() <= 1 {
            return;
        }
        let active_id = self
            .session
            .workspaces
            .get(self.session.active_workspace)
            .map(|entry| entry.id);
        self.session
            .workspaces
            .sort_by_key(|entry| (!entry.pinned, entry.id));
        if let Some(active_id) = active_id
            && let Some(next_active) = self
                .session
                .workspaces
                .iter()
                .position(|entry| entry.id == active_id)
        {
            self.session.active_workspace = next_active;
        }
    }

    fn stash_active_workspace_tabs(&mut self) {
        let active_tab = self.session.active_tab;
        if let Some(entry) = self
            .session
            .workspaces
            .get_mut(self.session.active_workspace)
        {
            entry.tabs = std::mem::take(&mut self.session.tabs);
            entry.active_tab = active_tab;
        }
    }

    fn restore_workspace_tabs(&mut self, index: usize) {
        if let Some(entry) = self.session.workspaces.get_mut(index) {
            self.session.tabs = std::mem::take(&mut entry.tabs);
            self.session.active_tab = entry
                .active_tab
                .min(self.session.tabs.len().saturating_sub(1));
            entry.attention = false;
        }
        self.session.active_workspace = index;
    }

    fn set_tab_scrollback_options(&self, tab_index: usize, active: bool) {
        let Some(inactive_scrollback) = self.inactive_tab_scrollback else {
            return;
        };
        let active_options = self.terminal_runtime.term_options();
        let options = if active {
            active_options
        } else {
            active_options.with_scrollback_history(inactive_scrollback)
        };
        if let Some(tab) = self.session.tabs.get(tab_index) {
            for pane in &tab.panes {
                pane.terminal().set_term_options(options);
            }
        }
    }

    fn reset_view_state_after_workspace_change(&mut self, cx: &mut Context<Self>) {
        self.reset_tab_rename_state();
        self.reset_workspace_rename_state();
        self.finish_workspace_drag();
        self.reset_tab_drag_state();
        self.clear_selection();
        self.clear_hovered_link();
        self.clear_terminal_scrollbar_marker_cache();
        self.refresh_search_if_open(cx);
        self.mark_tab_strip_layout_dirty();
        self.sync_tab_strip_for_active_tab();
        self.sync_plugin_lifecycle_state(false, cx);
        self.schedule_persist_native_workspace(cx);
        cx.notify();
    }

    pub(crate) fn switch_workspace(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.session.workspaces.len() || index == self.session.active_workspace {
            return;
        }
        if let Err(error) = self.materialize_pending_workspace(index) {
            log::error!("Failed to start saved workspace: {error}");
            crate::ui::toast::error("Failed to start saved workspace");
            self.notify_overlay(cx);
            return;
        }

        self.set_tab_scrollback_options(self.session.active_tab, false);
        self.stash_active_workspace_tabs();
        self.restore_workspace_tabs(index);
        self.set_tab_scrollback_options(self.session.active_tab, true);
        self.reset_view_state_after_workspace_change(cx);
    }

    fn replacement_workspace_index_before_deletion(
        removed_index: usize,
        workspace_count: usize,
    ) -> Option<usize> {
        if workspace_count <= 1 || removed_index >= workspace_count {
            return None;
        }
        if removed_index + 1 < workspace_count {
            Some(removed_index + 1)
        } else {
            Some(removed_index - 1)
        }
    }

    pub(in crate::terminal_view) fn delete_workspace_by_id(
        &mut self,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspace_delete_blocker(workspace_id).is_some() {
            return false;
        }
        let Some(removed_index) = self
            .session
            .workspaces
            .iter()
            .position(|entry| entry.id == workspace_id)
        else {
            return false;
        };
        let deleting_active = removed_index == self.session.active_workspace;
        let replacement_id = if deleting_active {
            let replacement_index = Self::replacement_workspace_index_before_deletion(
                removed_index,
                self.session.workspaces.len(),
            )
            .expect("deletable workspace must have a replacement");
            if let Err(error) = self.materialize_pending_workspace(replacement_index) {
                log::error!("Failed to start replacement workspace before deletion: {error}");
                crate::ui::toast::error("Could not start the replacement workspace");
                self.notify_overlay(cx);
                return false;
            }
            self.session.workspaces[replacement_index].id
        } else {
            self.session.workspaces[self.session.active_workspace].id
        };

        if let (Some(client), Some(pending)) = (
            self.multiplexer_client(),
            self.session.workspaces[removed_index]
                .pending_restore
                .as_ref(),
        ) {
            for id in pending
                .tabs
                .iter()
                .flat_map(|tab| &tab.panes)
                .filter_map(|pane| pane.session_id.as_deref())
            {
                if let Err(error) = client.close(id) {
                    crate::ui::toast::error(format!("Could not close workspace session: {error}"));
                    return false;
                }
            }
        }

        let removed_tabs = if deleting_active {
            self.session.tabs.as_slice()
        } else {
            self.session.workspaces[removed_index].tabs.as_slice()
        };
        let removed_pane_ids = removed_tabs
            .iter()
            .flat_map(|tab| tab.panes.iter().map(|pane| pane.id.clone()))
            .collect::<Vec<_>>();
        let removed_tab_ids = removed_tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();
        let _ = self.release_forwarded_mouse_presses_for_panes(&removed_pane_ids);
        for tab_id in removed_tab_ids {
            self.session.native_pane_zoom_snapshots.remove(&tab_id);
            self.session.native_pane_layout_trees.remove(&tab_id);
        }

        if deleting_active {
            self.session.tabs.clear();
            self.session.active_tab = 0;
        }
        self.session.workspaces.remove(removed_index);
        let replacement_index = self
            .session
            .workspaces
            .iter()
            .position(|entry| entry.id == replacement_id)
            .expect("workspace deletion must retain the selected replacement");
        if deleting_active {
            self.restore_workspace_tabs(replacement_index);
            self.set_tab_scrollback_options(self.session.active_tab, true);
        } else {
            self.session.active_workspace = replacement_index;
        }
        self.reset_view_state_after_workspace_change(cx);
        true
    }

    pub(crate) fn set_workspace_pinned(
        &mut self,
        index: usize,
        pinned: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(entry) = self.session.workspaces.get_mut(index) else {
            return false;
        };
        if entry.pinned == pinned {
            return false;
        }

        entry.pinned = pinned;
        self.finish_workspace_drag();
        self.reorder_workspaces_for_pins();
        self.mark_tab_strip_layout_dirty();
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    pub(crate) fn toggle_workspace_pinned(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let Some(pinned) = self.session.workspaces.get(index).map(|entry| entry.pinned) else {
            return false;
        };
        self.set_workspace_pinned(index, !pinned, cx)
    }

    pub(crate) fn reset_workspace_rename_state(&mut self) -> bool {
        let was_renaming = self.renaming_workspace.take().is_some();
        let had_text = !self.workspace_rename_input.text().is_empty();
        let was_selecting = self.inline_input_selecting;
        self.workspace_rename_input.clear();
        self.inline_input_selecting = false;
        let changed = was_renaming || had_text || was_selecting;
        if changed {
            self.reset_cursor_blink_phase();
        }
        changed
    }

    pub(crate) fn begin_rename_workspace(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.session.workspaces.len() {
            return;
        }

        if self.is_command_palette_open() {
            self.close_command_palette(cx);
        }
        if self.search_open {
            self.close_search(cx);
        }

        self.reset_tab_rename_state();
        self.finish_workspace_drag();
        self.renaming_workspace = Some(index);
        self.workspace_rename_input
            .set_text(self.workspace_display_name(index));
        self.reset_cursor_blink_phase();
        self.inline_input_selecting = false;
        cx.notify();
    }

    pub(crate) fn commit_rename_workspace(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.renaming_workspace else {
            return;
        };
        let trimmed = self.workspace_rename_input.text().trim().to_string();
        if let Some(entry) = self.session.workspaces.get_mut(index) {
            if trimmed.is_empty() {
                // Committing an empty name reverts to auto-titling.
                entry.custom_named = false;
            } else {
                entry.name = Self::truncate_tab_title(&trimmed);
                entry.custom_named = true;
            }
            self.schedule_persist_native_workspace(cx);
        }

        self.reset_workspace_rename_state();
        cx.notify();
    }

    pub(crate) fn cancel_rename_workspace(&mut self, cx: &mut Context<Self>) {
        if self.renaming_workspace.is_none() {
            return;
        }

        self.reset_workspace_rename_state();
        cx.notify();
    }

    pub(crate) fn add_workspace(&mut self, cx: &mut Context<Self>) {
        if self.runtime_kind() != RuntimeKind::Native {
            crate::ui::toast::info("Workspaces are not available with the tmux runtime");
            self.notify_overlay(cx);
            return;
        }

        let previous_workspace = self.session.active_workspace;
        let id = self.session.next_workspace_id;
        self.stash_active_workspace_tabs();
        self.session.workspaces.push(WorkspaceEntry::new(id));
        self.session.active_workspace = self.session.workspaces.len() - 1;
        self.session.active_tab = 0;
        self.add_tab(cx);
        if self.session.tabs.is_empty() {
            // Tab creation failed; roll back so no empty workspace survives.
            self.session.workspaces.pop();
            self.restore_workspace_tabs(previous_workspace);
            return;
        }
        self.session.next_workspace_id += 1;
        self.reset_view_state_after_workspace_change(cx);
    }

    /// Clear the active workspace's tabs while keeping the workspace itself.
    /// Used when the last tab of a workspace is closed or exits.
    pub(crate) fn close_active_workspace(&mut self, cx: &mut Context<Self>) {
        let removed_pane_ids = self
            .session
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter().map(|pane| pane.id.clone()))
            .collect::<Vec<_>>();
        let _ = self.release_forwarded_mouse_presses_for_panes(&removed_pane_ids);
        let removed_tab_ids = self
            .session
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>();
        for tab_id in removed_tab_ids {
            self.session.native_pane_zoom_snapshots.remove(&tab_id);
            self.session.native_pane_layout_trees.remove(&tab_id);
        }

        self.session.tabs.clear();
        self.session.active_tab = 0;
        if let Some(entry) = self
            .session
            .workspaces
            .get_mut(self.session.active_workspace)
        {
            entry.tabs.clear();
            entry.active_tab = 0;
        }
        self.reset_view_state_after_workspace_change(cx);
    }

    /// Merge every stashed workspace's tabs into the visible strip and keep a
    /// single workspace. Runs when the sidebar setting is turned off so no
    /// tab becomes unreachable.
    pub(crate) fn collapse_workspaces_into_active(&mut self, cx: &mut Context<Self>) {
        if !self.has_other_workspaces() {
            return;
        }
        for index in 0..self.session.workspaces.len() {
            if index == self.session.active_workspace {
                continue;
            }
            if let Err(error) = self.materialize_pending_workspace(index) {
                log::error!("Failed to start saved workspace before merging: {error}");
                crate::ui::toast::error("Could not merge saved workspaces");
                self.notify_overlay(cx);
                return;
            }
        }

        let mut merged: Vec<TerminalTab> = Vec::new();
        let mut active_tab = self.session.active_tab;
        let mut kept_entry: Option<WorkspaceEntry> = None;
        for (index, mut entry) in std::mem::take(&mut self.session.workspaces)
            .into_iter()
            .enumerate()
        {
            if index == self.session.active_workspace {
                active_tab = merged.len() + self.session.active_tab;
                merged.append(&mut self.session.tabs);
                kept_entry = Some(entry);
            } else {
                merged.append(&mut entry.tabs);
            }
        }

        self.session.active_tab = active_tab.min(merged.len().saturating_sub(1));
        self.session.tabs = merged;
        let mut entry = kept_entry.unwrap_or_else(|| WorkspaceEntry::new(1));
        entry.tabs = Vec::new();
        self.session.workspaces = vec![entry];
        self.session.active_workspace = 0;
        self.mark_tab_strip_layout_dirty();
        self.sync_tab_strip_for_active_tab();
        self.schedule_persist_native_workspace(cx);
    }

    /// Drain PTY events for tabs stashed in inactive workspaces so their
    /// processes never stall behind a full event channel. Only a minimal
    /// subset is applied (progress, cwd, exit, sidebar status); tab titles
    /// refresh on the next prompt after the workspace is reactivated.
    pub(super) fn drain_stashed_workspace_terminal_events(
        &mut self,
        cx: &mut Context<Self>,
        clipboard_text: &mut ClipboardTextCache,
        ready_terminal_ids: &mut HashSet<NativeTerminalWakeupId>,
    ) {
        let mut status_changed = false;
        let mut exited_panes: Vec<(u64, TabId, String)> = Vec::new();
        let explicit_prefix = self.tab_title.explicit_prefix.trim().to_string();
        let shell_integration_enabled = self.shell_integration_enabled;
        let wakeup_router = self.native_terminal_wakeup_router.clone();

        for entry in &mut self.session.workspaces {
            let mut attention = false;
            for tab in &mut entry.tabs {
                if ready_terminal_ids.is_empty() {
                    break;
                }
                let active_pane_id = tab.active_pane_id.clone();
                let mut running_update: Option<(bool, Option<String>)> = None;
                for pane in &mut tab.panes {
                    if ready_terminal_ids.is_empty() {
                        break;
                    }
                    let terminal = pane.terminal();
                    let Some(wakeup_id) = terminal.wakeup_id() else {
                        continue;
                    };
                    if !ready_terminal_ids.remove(&wakeup_id) {
                        continue;
                    }
                    let (events, has_more) = {
                        let mut reply_host = GpuiClipboardReplyHost::new(cx, clipboard_text);
                        terminal.drain_events(&mut reply_host)
                    };
                    if has_more {
                        wakeup_router.mark_ready(wakeup_id);
                    }
                    for event in events {
                        match event {
                            TerminalEvent::Exit => {
                                exited_panes.push((entry.id, tab.id, pane.id.clone()));
                            }
                            TerminalEvent::Progress(state) => {
                                pane.progress_state = state;
                            }
                            TerminalEvent::WorkingDirectory(path) => {
                                tab.last_prompt_cwd = Some(path);
                            }
                            TerminalEvent::Bell => {
                                attention = true;
                            }
                            TerminalEvent::ShellCommandFinished(_) => {
                                if shell_integration_enabled {
                                    attention = true;
                                }
                            }
                            // Track only the running-process flag from title
                            // events so the sidebar busy status stays fresh;
                            // the title text itself refreshes on reactivation.
                            TerminalEvent::Title(title) => {
                                if pane.id == active_pane_id
                                    && let Some(update) =
                                        Self::stashed_title_running_update(&explicit_prefix, &title)
                                {
                                    running_update = Some(update);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                if let Some((running_process, current_command)) = running_update {
                    if tab.running_process != running_process {
                        status_changed = true;
                    }
                    tab.running_process = running_process;
                    tab.current_command = current_command;
                }
            }
            if attention && !entry.attention {
                entry.attention = true;
                status_changed = true;
            }
            if ready_terminal_ids.is_empty() {
                break;
            }
        }

        for (workspace_id, tab_id, pane_id) in exited_panes {
            self.remove_stashed_pane(workspace_id, tab_id, pane_id.as_str(), cx);
        }

        if status_changed
            && (self.workspace_sidebar_visible() || self.workspace_sidebar_overlay_visible())
        {
            cx.notify();
        }
    }

    /// Minimal parse of an explicit-title event from a stashed tab: returns
    /// the `(running_process, current_command)` update it implies, if any.
    /// Mirrors the prompt/command prefixes of `parse_explicit_title` without
    /// touching title state.
    fn stashed_title_running_update(
        explicit_prefix: &str,
        title: &str,
    ) -> Option<(bool, Option<String>)> {
        if explicit_prefix.is_empty() {
            return None;
        }
        let payload = title.trim().strip_prefix(explicit_prefix)?.trim();
        if let Some(command) = payload.strip_prefix("command:") {
            let command = command.trim();
            if command.is_empty() {
                return None;
            }
            return Some((true, Some(command.to_string())));
        }
        if let Some(prompt) = payload.strip_prefix("prompt:") {
            if prompt.trim().is_empty() {
                return None;
            }
            return Some((false, None));
        }
        None
    }

    fn remove_stashed_pane(
        &mut self,
        workspace_id: u64,
        tab_id: TabId,
        pane_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace_index) = self
            .session
            .workspaces
            .iter()
            .position(|entry| entry.id == workspace_id)
        else {
            return;
        };
        if workspace_index == self.session.active_workspace {
            return;
        }

        let entry = &mut self.session.workspaces[workspace_index];
        let Some(tab_index) = entry.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let tab = &mut entry.tabs[tab_index];
        let Some(pane_index) = tab.panes.iter().position(|pane| pane.id == pane_id) else {
            return;
        };

        if tab.panes.len() > 1 {
            let removed = tab.panes.remove(pane_index);
            Self::native_close_expand_neighbors(&mut tab.panes, &removed);
            if tab.active_pane_id == removed.id || !tab.has_active_pane() {
                let next_index = pane_index.min(tab.panes.len().saturating_sub(1));
                if let Some(next) = tab.panes.get(next_index) {
                    tab.active_pane_id = next.id.clone();
                }
            }
            // The stored layout tree still references the removed leaf; drop
            // it so it is rebuilt from pane geometry on next use.
            self.session.native_pane_layout_trees.remove(&tab_id);
            self.session.native_pane_zoom_snapshots.remove(&tab_id);
            return;
        }

        entry.tabs.remove(tab_index);
        if entry.active_tab >= entry.tabs.len() {
            entry.active_tab = entry.tabs.len().saturating_sub(1);
        }
        self.session.native_pane_layout_trees.remove(&tab_id);
        self.session.native_pane_zoom_snapshots.remove(&tab_id);
        self.schedule_persist_native_workspace(cx);
    }

    /// All tabs held by inactive workspaces, in sidebar order.
    pub(super) fn stashed_workspace_tabs(&self) -> impl Iterator<Item = &TerminalTab> {
        self.session
            .workspaces
            .iter()
            .flat_map(|entry| entry.tabs.iter())
    }

    /// Stashed tabs that are busy (running process or fullscreen app), for
    /// quit warnings: the visible strip is checked separately.
    pub(crate) fn stashed_busy_workspace_tab_titles(&self) -> Vec<String> {
        let fallback_title = self.fallback_title();
        self.stashed_workspace_tabs()
            .filter(|tab| Self::tab_is_busy(tab))
            .map(|tab| {
                let title = tab.title.trim();
                if title.is_empty() {
                    fallback_title.to_string()
                } else {
                    title.to_string()
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_sidebar_width_clamps_to_configured_bounds() {
        assert_eq!(
            TerminalView::clamp_workspace_sidebar_width(1.0),
            termy_config_core::MIN_SIDEBAR_WIDTH
        );
        assert_eq!(TerminalView::clamp_workspace_sidebar_width(240.0), 240.0);
        assert_eq!(
            TerminalView::clamp_workspace_sidebar_width(10_000.0),
            termy_config_core::MAX_SIDEBAR_WIDTH
        );
        assert_eq!(
            TerminalView::clamp_workspace_sidebar_width(f32::NAN),
            termy_config_core::DEFAULT_SIDEBAR_WIDTH
        );
    }

    #[test]
    fn default_workspace_name_detection_matches_generated_names_only() {
        assert!(WorkspaceEntry::is_default_workspace_name("Workspace 1"));
        assert!(WorkspaceEntry::is_default_workspace_name("Workspace 42"));
        assert!(!WorkspaceEntry::is_default_workspace_name("Workspace "));
        assert!(!WorkspaceEntry::is_default_workspace_name("Workspace one"));
        assert!(!WorkspaceEntry::is_default_workspace_name("My Workspace 1"));
        assert!(!WorkspaceEntry::is_default_workspace_name("backend"));
        assert!(!WorkspaceEntry::is_default_workspace_name(""));
    }

    #[test]
    fn stashed_title_running_update_tracks_command_and_prompt_events() {
        assert_eq!(
            TerminalView::stashed_title_running_update("termy;", "termy; command: cargo build"),
            Some((true, Some("cargo build".to_string())))
        );
        assert_eq!(
            TerminalView::stashed_title_running_update("termy;", "termy; prompt: ~/dev"),
            Some((false, None))
        );
        assert_eq!(
            TerminalView::stashed_title_running_update("termy;", "termy; title: hello"),
            None
        );
        assert_eq!(
            TerminalView::stashed_title_running_update("termy;", "plain shell title"),
            None
        );
        assert_eq!(
            TerminalView::stashed_title_running_update("termy;", "termy; command:  "),
            None
        );
        assert_eq!(
            TerminalView::stashed_title_running_update("", "termy; command: cargo build"),
            None
        );
    }

    #[test]
    fn workspace_sidebar_toggle_respects_disabled_setting() {
        assert_eq!(
            TerminalView::next_workspace_sidebar_collapsed(false, false),
            None
        );
        assert_eq!(
            TerminalView::next_workspace_sidebar_collapsed(true, false),
            Some(true)
        );
        assert_eq!(
            TerminalView::next_workspace_sidebar_collapsed(true, true),
            Some(false)
        );
    }

    #[test]
    fn workspace_deletion_prefers_next_then_previous_replacement() {
        assert_eq!(
            TerminalView::replacement_workspace_index_before_deletion(0, 3),
            Some(1)
        );
        assert_eq!(
            TerminalView::replacement_workspace_index_before_deletion(1, 3),
            Some(2)
        );
        assert_eq!(
            TerminalView::replacement_workspace_index_before_deletion(2, 3),
            Some(1)
        );
    }

    #[test]
    fn workspace_deletion_refuses_missing_or_last_replacement() {
        assert_eq!(
            TerminalView::replacement_workspace_index_before_deletion(0, 1),
            None
        );
        assert_eq!(
            TerminalView::replacement_workspace_index_before_deletion(3, 3),
            None
        );
    }
}
