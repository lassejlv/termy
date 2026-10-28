//! Applying layout changes made by other clients (`termy mux`, other
//! windows) to an open window (audit B5), and pruning cold workspaces
//! against the host before they start (audit B15).
use super::*;
use crate::multiplexer::RemoteLayout;
use std::sync::atomic::AtomicU64;
use termy_core::session_model::{match_workspaces, rebase_session};

/// View IDs for panes attached because another client added them.
static NEXT_REMOTE_PANE_ID: AtomicU64 = AtomicU64::new(1);

impl TerminalView {
    /// Applies each layout the host publishes to this window, on the GPUI
    /// foreground, until the window closes.
    pub(in crate::terminal_view) fn start_multiplexer_layout_sync(&self, cx: &mut Context<Self>) {
        let Some(window) = &self.multiplexer else {
            return;
        };
        let changes = window.layout_changes();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(mut remote) = changes.recv_async().await {
                // Only the newest layout matters.
                while let Ok(newer) = changes.try_recv() {
                    remote = newer;
                }
                let applied = cx
                    .update(|cx| this.update(cx, |view, cx| view.apply_remote_layout(&remote, cx)));
                if applied.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Brings the view in line with `remote` when another client changed this
    /// window. Edits the view made since it last synced are rebased on top,
    /// and saved back if the result differs from what the host has.
    pub(in crate::terminal_view) fn apply_remote_layout(
        &mut self,
        remote: &RemoteLayout,
        cx: &mut Context<Self>,
    ) {
        let Some(window) = self.multiplexer.clone() else {
            return;
        };
        let Some((base, session)) = window.remote_change(remote) else {
            return;
        };
        let Some(local) = self.collect_stored_session() else {
            return;
        };
        let target = rebase_session(&base, &local, &session);
        let mut unattached = Vec::new();
        if target != local {
            unattached = self.mirror_stored_session(&local, target.clone(), cx);
        }
        // Saves collected before this change hold a session without it:
        // cancel them; the next save collects the view as it is now.
        self.native_persist_revision
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        // A pane that failed to attach is not in the view. Leave it out of
        // the base too, so the next save sees it as the host's, not as a pane
        // this view closed; the next layout change retries it.
        let mut adopted = session.clone();
        for id in &unattached {
            adopted.remove_pane(id);
        }
        window.adopt_remote(remote.revision, adopted);
        if target != session || !unattached.is_empty() {
            self.schedule_persist_native_workspace(cx);
        }
    }

    /// Rebuilds the workspaces and tabs from `target`, keeping every live
    /// pane, tab and workspace that `target` still lists (matched by host
    /// session ID), attaching panes it adds and detaching those it dropped.
    /// `local` is this view's own snapshot; its workspaces correspond to
    /// `self.session.workspaces` one to one. Returns the panes of `target`
    /// that could not be attached.
    fn mirror_stored_session(
        &mut self,
        local: &StoredSession,
        target: StoredSession,
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let mut unattached = Vec::new();
        if self.multiplexer.is_some() && target.zoom_steps != self.font_zoom_steps() {
            self.set_font_zoom_steps(target.zoom_steps, cx);
        }
        // Gather every live pane and tab, unzoomed, from all workspaces.
        self.set_tab_scrollback_options(self.session.active_tab, false);
        self.stash_active_workspace_tabs();
        let mut old_entries = std::mem::take(&mut self.session.workspaces);
        let mut panes: HashMap<String, TerminalPane> = HashMap::new();
        let mut tabs: HashMap<TabId, TerminalTab> = HashMap::new();
        let mut tab_of_pane: HashMap<String, TabId> = HashMap::new();
        let mut unknown = Vec::new();
        for entry in &mut old_entries {
            for mut tab in std::mem::take(&mut entry.tabs) {
                self.session.native_pane_layout_trees.remove(&tab.id);
                if let Some(zoom) = self.session.native_pane_zoom_snapshots.remove(&tab.id) {
                    Self::unzoom_panes(&mut tab, zoom);
                }
                for pane in std::mem::take(&mut tab.panes) {
                    match pane.terminal.session_id().map(str::to_owned) {
                        Some(id) => {
                            tab_of_pane.insert(id.clone(), tab.id);
                            panes.insert(id, pane);
                        }
                        None => unknown.push(pane),
                    }
                }
                tabs.insert(tab.id, tab);
            }
        }

        let matches = match_workspaces(&target.workspaces, &local.workspaces);
        let mut taken = vec![false; old_entries.len()];
        let mut entries = Vec::with_capacity(target.workspaces.len());
        for (index, stored) in target.workspaces.into_iter().enumerate() {
            let matched = matches[index].filter(|old| *old < old_entries.len() && !taken[*old]);
            let Some(old) = matched else {
                let id = self.session.next_workspace_id;
                self.session.next_workspace_id += 1;
                entries.push(Self::pending_workspace_entry_from_stored(id, stored));
                continue;
            };
            taken[old] = true;
            let mut entry =
                std::mem::replace(&mut old_entries[old], workspaces::WorkspaceEntry::new(0));
            if entry.pending_restore.is_some() {
                let mut pending = Self::pending_workspace_entry_from_stored(entry.id, stored);
                pending.attention = entry.attention;
                entries.push(pending);
                continue;
            }
            if !stored.name.is_empty() {
                entry.custom_named =
                    !workspaces::WorkspaceEntry::is_default_workspace_name(&stored.name);
                entry.name = stored.name.clone();
            }
            entry.pinned = stored.pinned;
            entry.active_tab = stored.active_tab;
            for stored_tab in stored.tabs {
                if let Some(tab) = self.mirror_tab(
                    stored_tab,
                    &mut panes,
                    &mut tabs,
                    &tab_of_pane,
                    &mut unattached,
                ) {
                    entry.tabs.push(tab);
                }
            }
            entry.active_tab = entry.active_tab.min(entry.tabs.len().saturating_sub(1));
            entries.push(entry);
        }

        // Whatever the host no longer lists in this window was closed there
        // or moved to another window: detach, never close.
        let dropped: Vec<TerminalPane> = panes.into_values().chain(unknown).collect();
        let dropped_ids: Vec<String> = dropped.iter().map(|pane| pane.id.clone()).collect();
        let _ = self.release_forwarded_mouse_presses_for_panes(&dropped_ids);
        let terminals = dropped
            .into_iter()
            .map(|pane| {
                pane.terminal.detach_session();
                pane.terminal
            })
            .collect();
        Self::dispose_native_terminals(terminals, cx);

        if entries.is_empty() {
            entries.push(workspaces::WorkspaceEntry::new(
                self.session.next_workspace_id,
            ));
            self.session.next_workspace_id += 1;
        }
        let active = target.active_workspace.min(entries.len() - 1);
        self.session.workspaces = entries;
        if let Err(error) = self.materialize_pending_workspace(active) {
            log::error!("Failed to start a workspace another client selected: {error}");
        }
        self.restore_workspace_tabs(active);
        self.set_tab_scrollback_options(self.session.active_tab, true);

        self.reset_tab_rename_state();
        self.reset_workspace_rename_state();
        self.finish_workspace_drag();
        self.reset_tab_drag_state();
        self.clear_terminal_scrollbar_marker_cache();
        self.refresh_search_if_open(cx);
        self.invalidate_native_split_generation();
        self.sync_native_terminal_wakeup_interest();
        // Programs in changed panes were sized by another client: fit them
        // to this view again on the next frame.
        self.last_terminal_resize_signature = None;
        self.last_resize_applied_at = None;
        self.finish_workspace_restore(cx);
        unattached
    }

    /// Builds one tab of `stored`, reusing the live tab and panes it still
    /// holds. Panes that cannot attach are left out; `None` if none remain.
    fn mirror_tab(
        &mut self,
        stored: StoredTab,
        panes: &mut HashMap<String, TerminalPane>,
        tabs: &mut HashMap<TabId, TerminalTab>,
        tab_of_pane: &HashMap<String, TabId>,
        unattached: &mut Vec<String>,
    ) -> Option<TerminalTab> {
        let (_, _, persisted) = Self::persisted_workspace_from_stored(StoredWorkspace {
            name: String::new(),
            pinned: false,
            active_tab: 0,
            tabs: vec![stored],
        });
        let persisted = persisted.tabs.into_iter().next()?;
        let mut built = Vec::with_capacity(persisted.panes.len());
        let mut active = None;
        for (index, saved) in persisted.panes.iter().enumerate() {
            // Live layouts always carry IDs; nothing else can attach.
            let Some(id) = saved.session_id.as_deref() else {
                continue;
            };
            let pane = match panes.remove(id) {
                Some(mut pane) => {
                    pane.left = saved.left;
                    pane.top = saved.top;
                    pane.width = saved.width.max(1);
                    pane.height = saved.height.max(1);
                    pane.pane_zoom_steps = saved.zoom_steps;
                    pane
                }
                None => match self.attach_remote_pane(id, saved) {
                    Ok(pane) => pane,
                    Err(error) => {
                        log::warn!("Cannot show pane {id} another client added: {error}");
                        unattached.push(id.to_owned());
                        continue;
                    }
                },
            };
            if index == persisted.active_pane {
                active = Some(pane.id.clone());
            }
            built.push(pane);
        }
        if built.is_empty() {
            return None;
        }
        let reused = persisted
            .panes
            .iter()
            .filter_map(|pane| tab_of_pane.get(pane.session_id.as_deref()?))
            .find_map(|tab_id| tabs.remove(tab_id));
        let mut tab = if let Some(mut tab) = reused {
            tab.panes = built;
            tab
        } else {
            let mut panes = built.into_iter();
            let first = panes.next()?;
            let mut tab = Self::create_restored_tab(
                self.allocate_tab_id(),
                first,
                None,
                persisted.manual_title.as_deref(),
            );
            tab.panes.extend(panes);
            if let Some(presentation) = persisted.presentation.clone() {
                tab.title = presentation.title;
                tab.explicit_title = presentation.explicit_title;
                tab.explicit_title_is_prediction = presentation.explicit_title_is_prediction;
                tab.shell_title = presentation.shell_title;
                tab.current_command = presentation.current_command;
                tab.last_prompt_cwd = presentation.last_prompt_cwd;
                tab.running_process = presentation.running_process;
            }
            tab
        };
        tab.active_pane_id = active
            .filter(|id| tab.panes.iter().any(|pane| &pane.id == id))
            .unwrap_or_else(|| tab.panes[0].id.clone());
        tab.pinned = persisted.pinned;
        tab.manual_title = persisted.manual_title.clone();
        let layout_tree = (tab.panes.len() == persisted.panes.len())
            .then_some(persisted.layout_tree.as_ref())
            .flatten();
        let (layout_tree, zoom) =
            Self::restored_tab_layout(&mut tab, layout_tree, persisted.zoomed);
        if let Some(layout_tree) = layout_tree {
            self.session
                .native_pane_layout_trees
                .insert(tab.id, layout_tree);
        }
        if let Some(zoom) = zoom {
            self.session.native_pane_zoom_snapshots.insert(tab.id, zoom);
        }
        Some(tab)
    }

    fn attach_remote_pane(
        &self,
        id: &str,
        saved: &PersistedNativePane,
    ) -> Result<TerminalPane, String> {
        let client = self
            .multiplexer_client()
            .ok_or_else(|| "the multiplexer is unavailable".to_string())?;
        let terminal = Terminal::attach_native_session(
            id,
            client,
            &self.native_terminal_wakeup_router,
            &self.terminal_runtime,
        )
        .map_err(|error| error.to_string())?;
        // This window now owns the pane: closing it here ends its process.
        terminal.adopt_session();
        let view_id = format!(
            "%native-remote-{}",
            NEXT_REMOTE_PANE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let mut pane = TerminalPane::new_native(
            view_id,
            saved.left,
            saved.top,
            saved.width.max(1),
            saved.height.max(1),
            terminal,
        );
        pane.pane_zoom_steps = saved.zoom_steps;
        Ok(pane)
    }

    /// Puts a zoomed tab's panes back in their split positions.
    pub(in crate::terminal_view) fn unzoom_panes(
        tab: &mut TerminalTab,
        zoom: NativePaneZoomSnapshot,
    ) {
        let mut panes = zoom.other_panes;
        for mut pane in std::mem::take(&mut tab.panes) {
            if pane.id == zoom.active_pane_id {
                (pane.left, pane.top, pane.width, pane.height) = zoom.active_pane_geometry;
                panes.insert(zoom.active_original_index.min(panes.len()), pane);
            } else {
                panes.push(pane);
            }
        }
        tab.panes = panes;
    }

    /// Drops panes the host closed, or that exited, from a workspace that has
    /// not started yet, collapsing split trees as a close would (audit B15).
    pub(in crate::terminal_view) fn prune_stored_workspace(
        &self,
        mut stored: StoredWorkspace,
    ) -> StoredWorkspace {
        let Some(client) = self.multiplexer_client() else {
            return stored;
        };
        let panes = match client.list() {
            Ok(panes) => panes,
            Err(error) => {
                log::warn!("Cannot check saved workspace panes: {error:#}");
                return stored;
            }
        };
        let gone: Vec<String> = stored
            .tabs
            .iter()
            .flat_map(|tab| &tab.panes)
            .filter_map(|pane| pane.session_id.clone())
            .filter(|id| {
                panes
                    .iter()
                    .find(|pane| &pane.id == id)
                    .is_none_or(|pane| pane.exited)
            })
            .collect();
        for id in gone {
            if panes.iter().any(|pane| pane.id == id) {
                // Exited: attaching would only replay its `Exit`.
                let _ = client.close(&id);
            }
            stored.remove_pane(&id);
        }
        stored
    }
}
