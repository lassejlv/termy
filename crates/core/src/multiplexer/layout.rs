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
                    for pane in panes {
                        if let Some(id) = pane.session_id {
                            self.resize(
                                &id,
                                crate::TerminalSize {
                                    cols: pane.width,
                                    rows: pane.height,
                                    cell_width: 9.0,
                                    cell_height: 18.0,
                                },
                            )?;
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
        if !self.supports_conditional_layout_updates() {
            return Ok(());
        }
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
                for pane in resized {
                    if let Some(id) = pane.session_id {
                        self.resize(
                            &id,
                            crate::TerminalSize {
                                cols: pane.width,
                                rows: pane.height,
                                cell_width: 9.0,
                                cell_height: 18.0,
                            },
                        )?;
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
        ensure!(
            self.supports_conditional_layout_updates(),
            "This running host does not support split layout editing"
        );
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
        launch.size.cols = pane.width;
        launch.size.rows = pane.height;
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
                    if let Some(id) = pane.session_id {
                        self.resize(
                            &id,
                            crate::TerminalSize {
                                cols: pane.width,
                                rows: pane.height,
                                cell_width: 9.0,
                                cell_height: 18.0,
                            },
                        )
                        .with_context(|| {
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
    /// Existing desktops keep their original create behavior on older hosts.
    pub fn create_tab(
        &self,
        launch: PaneLaunch,
        wakeup: Option<TerminalWakeupNotifier>,
        target: Option<WorkspaceTarget>,
    ) -> anyhow::Result<(String, Terminal)> {
        if !self.supports_conditional_layout_updates() {
            ensure!(
                target.is_none(),
                "This running host does not support workspace-targeted terminal creation"
            );
            return self.create(launch, wakeup);
        }
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
