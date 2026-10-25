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
        for _ in 0..8 {
            let before = self.layout()?;
            let mut layout = decode(before.as_deref())?;
            let window = layout
                .windows
                .iter_mut()
                .find(|w| w.id == window_id)
                .context("Window no longer exists")?;
            ensure!(
                &window.session == expected,
                "Window changed in another client; refresh before editing"
            );
            edit.apply(&mut window.session)
                .map_err(anyhow::Error::msg)?;
            if self.compare_and_set_layout(before, serde_json::to_string(&layout)?)? {
                return Ok(layout);
            }
        }
        anyhow::bail!("Session layout is changing; retry the window edit")
    }
    pub fn edit_tab(
        &self,
        pane_id: &str,
        expected: &StoredTab,
        edit: &TabEdit,
    ) -> anyhow::Result<StoredMultiplexer> {
        for _ in 0..8 {
            let before = self.layout()?;
            let mut layout = decode(before.as_deref())?;
            let tab = layout
                .windows
                .iter_mut()
                .flat_map(|w| &mut w.session.workspaces)
                .flat_map(|w| &mut w.tabs)
                .find(|tab| {
                    tab.panes
                        .iter()
                        .any(|pane| pane.session_id.as_deref() == Some(pane_id))
                })
                .context("Pane is not in a saved tab")?;
            ensure!(
                tab == expected,
                "Tab changed in another client; refresh before editing"
            );
            edit.apply(tab).map_err(anyhow::Error::msg)?;
            let panes = tab.panes.clone();
            if self.compare_and_set_layout(before, serde_json::to_string(&layout)?)? {
                if matches!(edit, TabEdit::ResizeDivider { .. }) {
                    let cell = self.tab_cell_size(&panes);
                    for pane in panes {
                        if let Some(id) = &pane.session_id {
                            self.resize(id, saved_size(&pane, cell))?;
                        }
                    }
                }
                return Ok(layout);
            }
        }
        anyhow::bail!("Session layout is changing; retry the tab edit")
    }
    pub fn close_saved_pane(&self, id: &str) -> anyhow::Result<()> {
        self.close(id)?;
        for _ in 0..8 {
            let expected = self.layout()?;
            let mut layout = decode(expected.as_deref())?;
            let mut changed = false;
            let mut resized = Vec::new();
            for workspace in layout
                .windows
                .iter_mut()
                .flat_map(|window| &mut window.session.workspaces)
            {
                let active_id = workspace
                    .tabs
                    .get(workspace.active_tab)
                    .and_then(|tab| {
                        tab.panes
                            .iter()
                            .filter_map(|pane| pane.session_id.as_ref())
                            .find(|pane_id| pane_id.as_str() != id)
                    })
                    .cloned();
                for tab in &mut workspace.tabs {
                    if tab
                        .remove_pane(id)
                        .map_err(anyhow::Error::msg)
                        .context("Pane closed, but its saved layout could not be repaired")?
                    {
                        changed = true;
                        resized.extend(tab.panes.clone());
                    }
                }
                workspace.tabs.retain(|tab| !tab.panes.is_empty());
                workspace.active_tab = active_id
                    .and_then(|id| {
                        workspace.tabs.iter().position(|tab| {
                            tab.panes
                                .iter()
                                .any(|pane| pane.session_id.as_deref() == Some(&id))
                        })
                    })
                    .unwrap_or(
                        workspace
                            .active_tab
                            .min(workspace.tabs.len().saturating_sub(1)),
                    );
            }
            if !changed {
                return Ok(());
            }
            if self.compare_and_set_layout(expected, serde_json::to_string(&layout)?)? {
                let cell = self.tab_cell_size(&resized);
                for pane in resized {
                    if let Some(id) = &pane.session_id {
                        self.resize(id, saved_size(&pane, cell))?;
                    }
                }
                return Ok(());
            }
        }
        anyhow::bail!(
            "Pane closed, but layout changes prevented cleanup; retry close to repair its saved layout"
        )
    }
    pub fn split_pane(
        &self,
        id: &str,
        axis: PaneResizeAxis,
        expected: &StoredTab,
        mut launch: PaneLaunch,
    ) -> anyhow::Result<(String, Terminal)> {
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
        let snapshot = decode(self.layout()?.as_deref())?;
        let tab = snapshot
            .windows
            .iter()
            .flat_map(|w| &w.session.workspaces)
            .flat_map(|w| &w.tabs)
            .find(|tab| {
                tab.panes
                    .iter()
                    .any(|pane| pane.session_id.as_deref() == Some(id))
            })
            .context("Pane is not in a saved tab")?;
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
        let result = (|| -> anyhow::Result<Vec<StoredPane>> {
            for _ in 0..8 {
                let before = self.layout()?;
                let mut layout = decode(before.as_deref())?;
                let tab = layout
                    .windows
                    .iter_mut()
                    .flat_map(|w| &mut w.session.workspaces)
                    .flat_map(|w| &mut w.tabs)
                    .find(|tab| {
                        tab.panes
                            .iter()
                            .any(|pane| pane.session_id.as_deref() == Some(id))
                    })
                    .context("Pane is not in a saved tab")?;
                ensure!(
                    tab == expected,
                    "Tab changed in another client; refresh before splitting"
                );
                tab.split_pane(
                    id,
                    StoredPane {
                        session_id: Some(new_id.clone()),
                        ..pane.clone()
                    },
                    axis,
                )
                .map_err(anyhow::Error::msg)?;
                let panes = tab.panes.clone();
                if self.compare_and_set_layout(before, serde_json::to_string(&layout)?)? {
                    return Ok(panes);
                }
            }
            anyhow::bail!("Session layout is changing; retry splitting")
        })();
        match result {
            Ok(panes) => {
                for pane in panes {
                    if let Some(id) = &pane.session_id {
                        self.resize(id, saved_size(&pane, cell)).with_context(|| {
                            format!("Split created pane {new_id}, but resizing pane {id} failed")
                        })?;
                    }
                }
            }
            Err(error) => {
                let saved=self.layout().with_context(||format!("{error}; pane {new_id} remains alive because its layout could not be verified"))?;
                let saved = decode(saved.as_deref())?;
                let registered = saved
                    .windows
                    .iter()
                    .flat_map(|w| &w.session.workspaces)
                    .flat_map(|w| &w.tabs)
                    .flat_map(|t| &t.panes)
                    .any(|pane| pane.session_id.as_deref() == Some(&new_id));
                if !registered {
                    self.close(&new_id)?;
                    return Err(error);
                }
            }
        }
        Ok((new_id, terminal))
    }
    /// Create a live terminal and register its tab in the selected workspace.
    pub fn create_tab(
        &self,
        launch: PaneLaunch,
        wakeup: Option<TerminalWakeupNotifier>,
        target: Option<WorkspaceTarget>,
    ) -> anyhow::Result<(String, Terminal)> {
        let size = launch.size;
        let initial = self.layout()?;
        let layout = decode(initial.as_deref())?;
        let target = match target {
            Some(target) => Some(target),
            None => layout.windows.first().and_then(|window| {
                let index = window
                    .session
                    .active_workspace
                    .min(window.session.workspaces.len().saturating_sub(1));
                window
                    .session
                    .workspaces
                    .get(index)
                    .map(|workspace| WorkspaceTarget {
                        window_id: window.id.clone(),
                        index,
                        expected: workspace.clone(),
                    })
            }),
        };
        if let Some(target) = &target {
            validate_target(&layout, target)?;
        }
        let (id, terminal) = self.create(launch, wakeup)?;
        let new_window_id = uuid::Uuid::new_v4().to_string();
        let result = (|| -> anyhow::Result<()> {
            for _ in 0..8 {
                let expected = self.layout()?;
                let mut layout = decode(expected.as_deref())?;
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
                if let Some(target) = &target {
                    validate_target(&layout, target)?;
                    let window = layout
                        .windows
                        .iter_mut()
                        .find(|window| window.id == target.window_id)
                        .unwrap();
                    let workspace = &mut window.session.workspaces[target.index];
                    workspace.active_tab = workspace.tabs.len();
                    workspace.tabs.push(tab);
                    window.session.active_workspace = target.index;
                } else {
                    layout.windows.push(StoredWindow {
                        id: new_window_id.clone(),
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
                    });
                }
                if self.compare_and_set_layout(expected, serde_json::to_string(&layout)?)? {
                    return Ok(());
                }
            }
            anyhow::bail!("Session layout is changing; retry creating the tab")
        })();
        if let Err(error) = result {
            // A response could be lost after a successful write. Re-read before
            // cleaning up, so an acknowledged layout never points at a killed pane.
            match self.layout() {
                Ok(snapshot) => {
                    let saved = decode(snapshot.as_deref())?;
                    let registered = saved
                        .windows
                        .iter()
                        .flat_map(|w| &w.session.workspaces)
                        .flat_map(|w| &w.tabs)
                        .flat_map(|t| &t.panes)
                        .any(|pane| pane.session_id.as_deref() == Some(&id));
                    if !registered {
                        self.close(&id).with_context(|| {
                            format!("{error}; could not clean up new pane {id}")
                        })?;
                        return Err(error);
                    }
                }
                Err(_) => anyhow::bail!(
                    "{error}; pane {id} remains alive because its saved state could not be verified"
                ),
            }
        }
        Ok((id, terminal))
    }
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
}

fn saved_size(pane: &StoredPane, (cell_width, cell_height): (f32, f32)) -> crate::TerminalSize {
    crate::TerminalSize {
        cols: pane.width,
        rows: pane.height,
        cell_width,
        cell_height,
    }
}

fn decode(json: Option<&str>) -> anyhow::Result<StoredMultiplexer> {
    let layout = json
        .map(serde_json::from_str)
        .transpose()?
        .unwrap_or(StoredMultiplexer {
            version: 1,
            windows: Vec::new(),
        });
    ensure!(layout.version == 1, "Unsupported session layout version");
    Ok(layout)
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
