use crate::multiplexer::{PaneLaunch, SessionClient};
use crate::session_model::*;
use crate::{Terminal, TerminalWakeupNotifier};
use anyhow::{Context, ensure};

impl SessionClient {
    pub fn edit_window(
        &self,
        window_id: &str,
        expected: &StoredSession,
        edit: &WindowEdit,
    ) -> anyhow::Result<StoredMultiplexer> {
        let update = self.update_layout(|layout| {
            let window = layout
                .windows
                .iter_mut()
                .find(|w| w.id == window_id)
                .context("Window no longer exists")?;
            ensure!(
                &window.session == expected,
                "Window changed in another client; refresh before editing"
            );
            edit.apply(&mut window.session).map_err(anyhow::Error::msg)
        })?;
        Ok(update.layout)
    }

    /// Applies `edit` to the tab holding `pane_id`. A divider resize then
    /// resizes the tab's terminals; those resizes cannot undo the committed
    /// edit, so their failures are returned as warnings (audit L7).
    pub fn edit_tab(
        &self,
        pane_id: &str,
        expected: &StoredTab,
        edit: &TabEdit,
    ) -> anyhow::Result<(StoredMultiplexer, Vec<String>)> {
        let update = self.update_layout(|layout| {
            let tab = find_tab_mut(layout, pane_id).context("Pane is not in a saved tab")?;
            ensure!(
                tab == expected,
                "Tab changed in another client; refresh before editing"
            );
            edit.apply(tab).map_err(anyhow::Error::msg)?;
            Ok(tab.panes.clone())
        })?;
        let warnings = if matches!(edit, TabEdit::ResizeDivider { .. }) {
            self.resize_saved_panes(&update.value)
        } else {
            Vec::new()
        };
        Ok((update.layout, warnings))
    }

    /// Removes the pane from the saved layout, then terminates it (audit L6):
    /// a failed close never leaves the layout listing a dead pane, and a tab
    /// whose split tree cannot be inferred is re-tiled rather than failing.
    /// Resizing the surviving panes is best effort; failures are warnings.
    pub fn close_saved_pane(&self, id: &str) -> anyhow::Result<Vec<String>> {
        let survivors = self
            .update_layout(|layout| {
                let sibling = find_tab_mut(layout, id).and_then(|tab| {
                    tab.panes
                        .iter()
                        .filter_map(|pane| pane.session_id.clone())
                        .find(|pane| pane != id)
                });
                layout.remove_pane(id);
                Ok(sibling
                    .and_then(|sibling| find_tab_mut(layout, &sibling))
                    .map(|tab| tab.panes.clone())
                    .unwrap_or_default())
            })?
            .value;
        self.close(id)?;
        Ok(self.resize_saved_panes(&survivors))
    }

    /// Splits pane `id`. `expected` is the tab as the caller last saw it: it
    /// must still match before a process starts. Once the new pane runs, it is
    /// added to whatever the tab looks like then, so a concurrent save cannot
    /// make the split fail and kill it. Resizing the tab's panes afterwards is
    /// best effort; failures are returned as warnings (audit L7).
    pub fn split_pane(
        &self,
        id: &str,
        axis: PaneResizeAxis,
        expected: &StoredTab,
        mut launch: PaneLaunch,
    ) -> anyhow::Result<(String, Terminal, Vec<String>)> {
        let pane = expected
            .panes
            .iter()
            .find(|pane| pane.session_id.as_deref() == Some(id))
            .context("Pane is not in the expected tab")?;
        let mut probe = expected.clone();
        let probe_id = uuid::Uuid::new_v4().to_string();
        probe
            .split_pane(
                id,
                StoredPane {
                    session_id: Some(probe_id),
                    ..pane.clone()
                },
                axis,
            )
            .map_err(anyhow::Error::msg)?;
        let mut snapshot =
            StoredMultiplexer::parse(self.layout()?.as_deref()).map_err(anyhow::Error::msg)?;
        let tab = find_tab_mut(&mut snapshot, id).context("Pane is not in a saved tab")?;
        ensure!(
            tab == expected,
            "Tab changed in another client; refresh before splitting"
        );
        // The new pane shares the split pane's font, so its live metrics are
        // more accurate than a launch made without a UI.
        let cell = self
            .cell_size(id)
            .unwrap_or((launch.size.cell_width, launch.size.cell_height));
        launch.size = saved_size(pane, cell);
        let (new_id, terminal) = self.create(launch, None)?;
        let result = self.update_layout(|layout| {
            // A start-up reconcile elsewhere may already list the new pane.
            layout.remove_pane(&new_id);
            let tab = find_tab_mut(layout, id).context("Pane is not in a saved tab")?;
            let source = tab
                .panes
                .iter()
                .find(|pane| pane.session_id.as_deref() == Some(id))
                .cloned()
                .context("Pane is not in a saved tab")?;
            tab.split_pane(
                id,
                StoredPane {
                    session_id: Some(new_id.clone()),
                    ..source
                },
                axis,
            )
            .map_err(anyhow::Error::msg)?;
            Ok(tab.panes.clone())
        });
        let warnings = match result {
            Ok(update) => self.resize_saved_panes_with(&update.value, cell),
            Err(error) => {
                self.forget_unregistered_pane(&new_id, error)?;
                Vec::new()
            }
        };
        Ok((new_id, terminal, warnings))
    }

    /// Create a live terminal and register its tab in the selected workspace.
    /// A `target` must still match its expected workspace before the process
    /// starts. Afterwards the tab goes into that workspace (found again by its
    /// panes or name if it moved), or into the first window's active
    /// workspace, as it is at that moment: other clients' edits meanwhile
    /// only cause a retry (audit B17).
    pub fn create_tab(
        &self,
        launch: PaneLaunch,
        wakeup: Option<TerminalWakeupNotifier>,
        target: Option<WorkspaceTarget>,
    ) -> anyhow::Result<(String, Terminal)> {
        let placement = match target {
            Some(target) => {
                let layout = StoredMultiplexer::parse(self.layout()?.as_deref())
                    .map_err(anyhow::Error::msg)?;
                validate_target(&layout, &target)?;
                Placement::Workspace {
                    window_id: target.window_id,
                    index: target.index,
                    snapshot: target.expected,
                }
            }
            None => Placement::FirstWindow,
        };
        self.create_tab_at(launch, wakeup, placement)
    }

    /// Like [`Self::create_tab`] into workspace `index` of `window_id`, for a
    /// caller with no expectations of its own (`termy mux create --window`):
    /// only that workspace must still exist, wherever it moved (audit B17).
    pub fn create_tab_in_workspace(
        &self,
        launch: PaneLaunch,
        wakeup: Option<TerminalWakeupNotifier>,
        window_id: &str,
        index: usize,
    ) -> anyhow::Result<(String, Terminal)> {
        let layout =
            StoredMultiplexer::parse(self.layout()?.as_deref()).map_err(anyhow::Error::msg)?;
        let snapshot = layout
            .windows
            .iter()
            .find(|window| window.id == window_id)
            .and_then(|window| window.session.workspaces.get(index))
            .context("Workspace does not exist")?
            .clone();
        self.create_tab_at(
            launch,
            wakeup,
            Placement::Workspace {
                window_id: window_id.to_owned(),
                index,
                snapshot,
            },
        )
    }

    fn create_tab_at(
        &self,
        launch: PaneLaunch,
        wakeup: Option<TerminalWakeupNotifier>,
        placement: Placement,
    ) -> anyhow::Result<(String, Terminal)> {
        let size = launch.size;
        let (id, terminal) = self.create(launch, wakeup)?;
        let new_window_id = uuid::Uuid::new_v4().to_string();
        let tab = StoredTab {
            zoomed: false,
            presentation: None,
            pinned: false,
            manual_title: None,
            active_pane: 0,
            layout_tree_json: None,
            panes: vec![StoredPane {
                session_id: Some(id.clone()),
                left: 0,
                top: 0,
                width: size.cols,
                height: size.rows,
                buffer: None,
                zoom_steps: 0,
            }],
        };
        let result = self
            .update_layout(|layout| place_new_tab(layout, &placement, tab.clone(), &new_window_id));
        if let Err(error) = result {
            self.forget_unregistered_pane(&id, error)?;
        }
        Ok((id, terminal))
    }
}

/// Where [`SessionClient::create_tab`] puts a new tab.
enum Placement {
    /// The first window's active workspace, or a new window.
    FirstWindow,
    /// The workspace that was at `index` of `window_id` as `snapshot`.
    Workspace {
        window_id: String,
        index: usize,
        snapshot: StoredWorkspace,
    },
}

/// Adds `tab`, holding one new pane, to the layout.
fn place_new_tab(
    layout: &mut StoredMultiplexer,
    placement: &Placement,
    tab: StoredTab,
    new_window_id: &str,
) -> anyhow::Result<()> {
    // Another client's start-up reconcile may already list the new pane as a
    // recovered session; move it rather than listing it twice.
    for id in tab
        .panes
        .iter()
        .filter_map(|pane| pane.session_id.as_deref())
    {
        layout.remove_pane(id);
    }
    let target = match placement {
        Placement::Workspace {
            window_id,
            index,
            snapshot,
        } => {
            let window = layout
                .windows
                .iter()
                .find(|window| &window.id == window_id)
                .context("Window no longer exists")?;
            let index = locate_workspace(&window.session.workspaces, *index, snapshot)
                .context("Workspace no longer exists")?;
            Some((window_id.clone(), index))
        }
        Placement::FirstWindow => layout.windows.first().map(|window| {
            let index = window
                .session
                .active_workspace
                .min(window.session.workspaces.len().saturating_sub(1));
            (window.id.clone(), index)
        }),
    };
    match target {
        Some((window_id, index)) => {
            let window = layout
                .windows
                .iter_mut()
                .find(|window| window.id == window_id)
                .context("Window no longer exists")?;
            let workspace = window
                .session
                .workspaces
                .get_mut(index)
                .context("Workspace no longer exists")?;
            workspace.active_tab = workspace.tabs.len();
            workspace.tabs.push(tab);
            window.session.active_workspace = index;
        }
        None => layout.windows.push(StoredWindow {
            id: new_window_id.to_owned(),
            session: StoredSession {
                active_workspace: 0,
                zoom_steps: 0,
                workspaces: vec![StoredWorkspace {
                    name: "Workspace 1".into(),
                    pinned: false,
                    active_tab: 0,
                    tabs: vec![tab],
                }],
            },
        }),
    }
    Ok(())
}

/// Finds the workspace that was `snapshot` at `index`: by a pane it held,
/// else at `index` if the name still matches, else by its name. `None` once
/// it was deleted.
fn locate_workspace(
    workspaces: &[StoredWorkspace],
    index: usize,
    snapshot: &StoredWorkspace,
) -> Option<usize> {
    let ids: Vec<&str> = snapshot.session_ids().collect();
    if let Some(position) = workspaces
        .iter()
        .position(|workspace| workspace.session_ids().any(|id| ids.contains(&id)))
    {
        return Some(position);
    }
    if workspaces
        .get(index)
        .is_some_and(|workspace| workspace.name == snapshot.name)
    {
        return Some(index);
    }
    (!snapshot.name.is_empty())
        .then(|| {
            workspaces
                .iter()
                .position(|workspace| workspace.name == snapshot.name)
        })
        .flatten()
}

impl SessionClient {
    /// Cell metrics for resizing a tab's panes. They share one font, so the
    /// metrics the attached UI applied to any live pane hold for all of them;
    /// the defaults are only a fallback when no pane can be queried.
    fn tab_cell_size(&self, panes: &[StoredPane]) -> (f32, f32) {
        let default = crate::TerminalSize::default();
        panes
            .iter()
            .filter_map(|pane| pane.session_id.as_deref())
            .find_map(|id| self.cell_size(id).ok())
            .unwrap_or((default.cell_width, default.cell_height))
    }

    fn resize_saved_panes(&self, panes: &[StoredPane]) -> Vec<String> {
        self.resize_saved_panes_with(panes, self.tab_cell_size(panes))
    }

    /// Resizes each pane to its saved geometry and reports the failures.
    fn resize_saved_panes_with(&self, panes: &[StoredPane], cell: (f32, f32)) -> Vec<String> {
        panes
            .iter()
            .filter_map(|pane| {
                let id = pane.session_id.as_deref()?;
                self.resize(id, saved_size(pane, cell))
                    .err()
                    .map(|error| format!("Resizing pane {id} failed: {error:#}"))
            })
            .collect()
    }

    /// After a failed layout update, closes the new pane `id` unless the
    /// layout lists it after all (a reply can be lost after the write), and
    /// returns `error`.
    fn forget_unregistered_pane(&self, id: &str, error: anyhow::Error) -> anyhow::Result<()> {
        let Ok(snapshot) = self.layout() else {
            anyhow::bail!(
                "{error}; pane {id} remains alive because its saved state could not be verified"
            );
        };
        let saved = StoredMultiplexer::parse(snapshot.as_deref()).map_err(anyhow::Error::msg)?;
        if saved.session_ids().any(|pane| pane == id) {
            return Ok(());
        }
        self.close(id)
            .with_context(|| format!("{error}; could not clean up new pane {id}"))?;
        Err(error)
    }
}

fn find_tab_mut<'a>(layout: &'a mut StoredMultiplexer, id: &str) -> Option<&'a mut StoredTab> {
    layout
        .windows
        .iter_mut()
        .flat_map(|w| &mut w.session.workspaces)
        .flat_map(|w| &mut w.tabs)
        .find(|tab| {
            tab.panes
                .iter()
                .any(|pane| pane.session_id.as_deref() == Some(id))
        })
}

fn saved_size(pane: &StoredPane, (cell_width, cell_height): (f32, f32)) -> crate::TerminalSize {
    crate::TerminalSize {
        cols: pane.width,
        rows: pane.height,
        cell_width,
        cell_height,
    }
}

fn validate_target(layout: &StoredMultiplexer, target: &WorkspaceTarget) -> anyhow::Result<()> {
    let workspace = layout
        .windows
        .iter()
        .find(|window| window.id == target.window_id)
        .and_then(|window| window.session.workspaces.get(target.index))
        .context("Workspace no longer exists")?;
    ensure!(
        workspace == &target.expected,
        "Workspace changed in another client; refresh before creating a tab"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str) -> StoredTab {
        StoredTab {
            zoomed: false,
            presentation: None,
            pinned: false,
            manual_title: None,
            active_pane: 0,
            layout_tree_json: None,
            panes: vec![StoredPane {
                session_id: Some(id.into()),
                left: 0,
                top: 0,
                width: 80,
                height: 24,
                buffer: None,
                zoom_steps: 0,
            }],
        }
    }

    fn workspace(name: &str, tabs: Vec<StoredTab>) -> StoredWorkspace {
        StoredWorkspace {
            name: name.into(),
            pinned: false,
            active_tab: 0,
            tabs,
        }
    }

    fn window(workspaces: Vec<StoredWorkspace>) -> StoredMultiplexer {
        StoredMultiplexer {
            version: STORED_MULTIPLEXER_VERSION,
            windows: vec![StoredWindow {
                id: "window".into(),
                session: StoredSession {
                    workspaces,
                    active_workspace: 0,
                    zoom_steps: 0,
                },
            }],
        }
    }

    /// Audit B17: the target workspace is found again after it moved, and a
    /// deleted one fails clearly instead of receiving the tab elsewhere.
    #[test]
    fn workspaces_are_located_by_panes_then_names() {
        let snapshot = workspace("Work", vec![tab("a")]);
        let moved = [
            workspace("Review", Vec::new()),
            workspace("Renamed", vec![tab("a")]),
        ];
        assert_eq!(locate_workspace(&moved, 0, &snapshot), Some(1));
        let empty = workspace("Review", Vec::new());
        let reordered = [workspace("Work", vec![tab("a")]), empty.clone()];
        assert_eq!(locate_workspace(&reordered, 0, &empty), Some(1));
        let unnamed = workspace("", Vec::new());
        assert_eq!(
            locate_workspace(std::slice::from_ref(&unnamed), 0, &unnamed),
            Some(0)
        );
        assert_eq!(
            locate_workspace(&[workspace("Other", Vec::new())], 0, &empty),
            None
        );
    }

    /// Placing a new pane that a start-up reconcile already listed as a
    /// recovered session moves it instead of listing it twice.
    #[test]
    fn a_new_pane_already_recovered_is_moved_not_duplicated() {
        let mut layout = window(vec![
            workspace("Work", Vec::new()),
            workspace("Recovered sessions", vec![tab("new")]),
        ]);
        let placement = Placement::Workspace {
            window_id: "window".into(),
            index: 0,
            snapshot: workspace("Work", Vec::new()),
        };
        place_new_tab(&mut layout, &placement, tab("new"), "unused").unwrap();
        assert_eq!(layout.session_ids().collect::<Vec<_>>(), ["new"]);
        assert_eq!(layout.windows[0].session.workspaces[0].tabs.len(), 1);
        assert_eq!(layout.windows[0].session.active_workspace, 0);
    }
}
