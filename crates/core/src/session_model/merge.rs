use crate::session_model::*;

fn field<T: Clone + PartialEq>(base: &T, local: &T, remote: &T) -> Result<T, &'static str> {
    if local == base || local == remote {
        Ok(remote.clone())
    } else if remote == base {
        Ok(local.clone())
    } else {
        Err("The same session setting changed in another client")
    }
}

fn sequence<T: Clone + PartialEq, I: PartialEq>(
    base: &[T],
    local: &[T],
    remote: &[T],
    identity: impl Fn(&T) -> I,
    merge: impl Fn(&T, &T, &T) -> Result<T, &'static str>,
) -> Result<Vec<T>, &'static str> {
    if local == base || local == remote {
        return Ok(remote.to_vec());
    }
    if remote == base {
        return Ok(local.to_vec());
    }
    if base.len() != local.len()
        || base.len() != remote.len()
        || !base
            .iter()
            .zip(local)
            .zip(remote)
            .all(|((base, local), remote)| {
                identity(base) == identity(local) && identity(base) == identity(remote)
            })
    {
        return Err("The session layout changed in another client");
    }
    base.iter()
        .zip(local)
        .zip(remote)
        .map(|((base, local), remote)| merge(base, local, remote))
        .collect()
}

fn tab_identity(tab: &StoredTab) -> Vec<Option<String>> {
    tab.panes
        .iter()
        .map(|pane| pane.session_id.clone())
        .collect()
}

fn workspace_identity(workspace: &StoredWorkspace) -> (Vec<Vec<Option<String>>>, Option<String>) {
    (
        workspace.tabs.iter().map(tab_identity).collect(),
        workspace.tabs.is_empty().then(|| workspace.name.clone()),
    )
}

/// Merge edits relative to the caller's last local snapshot. Unchanged local
/// fields preserve remote edits. Structural conflicts fail rather than joining
/// unrelated panes by array index. Callers must publish with compare-and-set.
pub fn merge_session(
    base: &StoredSession,
    local: &StoredSession,
    remote: &StoredSession,
) -> Result<StoredSession, &'static str> {
    if local == base {
        return Ok(remote.clone());
    }
    if remote == base || local == remote {
        return Ok(local.clone());
    }
    // A single workspace cannot be reordered. Its empty-state name is
    // mutable metadata, whereas names disambiguate multiple empty workspaces.
    let identity = |workspace: &StoredWorkspace| {
        let (tabs, name) = workspace_identity(workspace);
        (
            tabs,
            if base.workspaces.len() == 1 {
                None
            } else {
                name
            },
        )
    };
    // Active indices cannot be applied across a remote reorder or insertion.
    let base_order: Vec<_> = base.workspaces.iter().map(identity).collect();
    let remote_order: Vec<_> = remote.workspaces.iter().map(identity).collect();
    let local_order: Vec<_> = local.workspaces.iter().map(identity).collect();
    if (local.active_workspace != base.active_workspace && base_order != remote_order)
        || (remote.active_workspace != base.active_workspace && base_order != local_order)
    {
        return Err("The workspace order changed in another client");
    }
    Ok(StoredSession {
        active_workspace: field(
            &base.active_workspace,
            &local.active_workspace,
            &remote.active_workspace,
        )?,
        workspaces: sequence(
            &base.workspaces,
            &local.workspaces,
            &remote.workspaces,
            identity,
            merge_workspace,
        )?,
    })
}

fn merge_workspace(
    base: &StoredWorkspace,
    local: &StoredWorkspace,
    remote: &StoredWorkspace,
) -> Result<StoredWorkspace, &'static str> {
    let base_order: Vec<_> = base.tabs.iter().map(tab_identity).collect();
    let local_order: Vec<_> = local.tabs.iter().map(tab_identity).collect();
    let remote_order: Vec<_> = remote.tabs.iter().map(tab_identity).collect();
    if (local.active_tab != base.active_tab && base_order != remote_order)
        || (remote.active_tab != base.active_tab && base_order != local_order)
    {
        return Err("The tab order changed in another client");
    }
    Ok(StoredWorkspace {
        name: field(&base.name, &local.name, &remote.name)?,
        pinned: field(&base.pinned, &local.pinned, &remote.pinned)?,
        active_tab: field(&base.active_tab, &local.active_tab, &remote.active_tab)?,
        tabs: sequence(
            &base.tabs,
            &local.tabs,
            &remote.tabs,
            tab_identity,
            merge_tab,
        )?,
    })
}

fn merge_tab(
    base: &StoredTab,
    local: &StoredTab,
    remote: &StoredTab,
) -> Result<StoredTab, &'static str> {
    let (active_pane, layout_tree_json, panes) = field(
        &(base.active_pane, &base.layout_tree_json, &base.panes),
        &(local.active_pane, &local.layout_tree_json, &local.panes),
        &(remote.active_pane, &remote.layout_tree_json, &remote.panes),
    )?;
    Ok(StoredTab {
        zoomed: field(&base.zoomed, &local.zoomed, &remote.zoomed)?,
        pinned: field(&base.pinned, &local.pinned, &remote.pinned)?,
        manual_title: field(
            &base.manual_title,
            &local.manual_title,
            &remote.manual_title,
        )?,
        presentation: field(
            &base.presentation,
            &local.presentation,
            &remote.presentation,
        )?,
        active_pane,
        // Geometry and the split tree must always describe the same layout.
        layout_tree_json: layout_tree_json.clone(),
        panes: panes.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> StoredSession {
        StoredSession {
            active_workspace: 0,
            workspaces: vec![StoredWorkspace {
                name: "Development".into(),
                pinned: false,
                active_tab: 0,
                tabs: vec![StoredTab {
                    zoomed: false,
                    presentation: None,
                    pinned: false,
                    manual_title: None,
                    active_pane: 0,
                    layout_tree_json: None,
                    panes: vec![StoredPane {
                        session_id: Some("one".into()),
                        left: 0,
                        top: 0,
                        width: 80,
                        height: 24,
                        buffer: None,
                    }],
                }],
            }],
        }
    }
    #[test]
    fn independent_edits_survive_repeated_desktop_saves() {
        let base = session();
        let mut local = base.clone();
        local.workspaces[0].tabs[0].manual_title = Some("Server".into());
        let mut remote = base.clone();
        remote.workspaces[0].pinned = true;
        let merged = merge_session(&base, &local, &remote).unwrap();
        assert!(merged.workspaces[0].pinned);
        assert_eq!(
            merged.workspaces[0].tabs[0].manual_title.as_deref(),
            Some("Server")
        );
        assert_eq!(merge_session(&local, &local, &merged).unwrap(), merged);
    }
    #[test]
    fn conflicting_edits_and_structural_changes_do_not_merge() {
        let base = session();
        let mut local = base.clone();
        let mut remote = base.clone();
        local.workspaces[0].name = "Local".into();
        remote.workspaces[0].name = "Remote".into();
        assert!(merge_session(&base, &local, &remote).is_err());
        remote = base.clone();
        remote.workspaces[0].tabs[0].panes[0].session_id = Some("replacement".into());
        assert!(merge_session(&base, &local, &remote).is_err());
    }
}
