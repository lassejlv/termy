//! Rebasing a window's local layout onto the host's after both changed its
//! structure (audit B3). Panes are matched by their host session ID; tabs and
//! workspaces by the panes they hold.
use crate::session_model::*;
use std::collections::HashSet;

/// Merges like [`merge_session`] and, where that reports a conflict, rebases
/// the local edits onto `remote` instead of failing:
///
/// - The host decides which panes exist: panes it no longer lists are gone.
/// - Panes the view closed since `base` are removed; panes and tabs it opened
///   are added at their local position. When both sides changed the same
///   tab's panes, the host's split wins and the view's new panes become a tab
///   of their own.
/// - Workspaces the view created are appended; empty workspaces it deleted are
///   removed.
/// - Renames, pins, zoom and selections the view changed since `base` win;
///   everything else follows `remote`, including the order of tabs and
///   workspaces.
pub fn rebase_session(
    base: &StoredSession,
    local: &StoredSession,
    remote: &StoredSession,
) -> StoredSession {
    match merge_session(base, local, remote) {
        Ok(merged) => merged,
        Err(_) => Rebase::new(base, local, remote).run(),
    }
}

/// Pairs each workspace of `from` with at most one workspace of `to`: first by
/// shared panes, then by equal non-empty names, then by position when both
/// lists have the same length and at least one of the two names is empty.
pub fn match_workspaces(from: &[StoredWorkspace], to: &[StoredWorkspace]) -> Vec<Option<usize>> {
    let mut matches = vec![None; from.len()];
    let mut taken = vec![false; to.len()];
    for (index, workspace) in from.iter().enumerate() {
        let ids: HashSet<&str> = workspace.session_ids().collect();
        let best = to
            .iter()
            .enumerate()
            .filter(|(candidate, _)| !taken[*candidate])
            .map(|(candidate, other)| {
                let shared = other.session_ids().filter(|id| ids.contains(id)).count();
                (shared, std::cmp::Reverse(candidate))
            })
            .filter(|(shared, _)| *shared > 0)
            .max();
        if let Some((_, std::cmp::Reverse(candidate))) = best {
            matches[index] = Some(candidate);
            taken[candidate] = true;
        }
    }
    for (index, workspace) in from.iter().enumerate() {
        if matches[index].is_some() || workspace.name.is_empty() {
            continue;
        }
        if let Some(candidate) = (0..to.len())
            .find(|candidate| !taken[*candidate] && to[*candidate].name == workspace.name)
        {
            matches[index] = Some(candidate);
            taken[candidate] = true;
        }
    }
    if from.len() == to.len() {
        // Two different user-chosen names never denote the same workspace.
        for (index, matched) in matches.iter_mut().enumerate() {
            let (left, right) = (&from[index].name, &to[index].name);
            if matched.is_none() && !taken[index] && (left.is_empty() || right.is_empty()) {
                *matched = Some(index);
                taken[index] = true;
            }
        }
    }
    matches
}

struct Rebase<'a> {
    base: &'a StoredSession,
    local: &'a StoredSession,
    base_ids: HashSet<&'a str>,
    remote_ids: HashSet<&'a str>,
    result: StoredSession,
}

impl<'a> Rebase<'a> {
    fn new(base: &'a StoredSession, local: &'a StoredSession, remote: &'a StoredSession) -> Self {
        Self {
            base,
            local,
            base_ids: base.session_ids().collect(),
            remote_ids: remote.session_ids().collect(),
            result: remote.clone(),
        }
    }

    /// A pane this view opened since it last synced.
    fn added(&self, id: &str) -> bool {
        !self.base_ids.contains(id) && !self.remote_ids.contains(id)
    }

    fn run(mut self) -> StoredSession {
        self.remove_local_closures();
        self.remove_local_workspace_deletions();
        self.add_local_tabs();
        self.apply_local_edits();
        let mut result = self.result;
        result.active_workspace = result
            .active_workspace
            .min(result.workspaces.len().saturating_sub(1));
        for workspace in &mut result.workspaces {
            workspace.active_tab = workspace
                .active_tab
                .min(workspace.tabs.len().saturating_sub(1));
            for tab in &mut workspace.tabs {
                tab.active_pane = tab.active_pane.min(tab.panes.len().saturating_sub(1));
            }
        }
        result
    }

    /// Panes the view closed (or moved to another window) that the host
    /// still lists.
    fn remove_local_closures(&mut self) {
        let local_ids: HashSet<&str> = self.local.session_ids().collect();
        let closed: Vec<&str> = self
            .base_ids
            .iter()
            .copied()
            .filter(|id| !local_ids.contains(id) && self.remote_ids.contains(id))
            .collect();
        for id in closed {
            self.result.remove_pane(id);
        }
    }

    /// Workspaces the view deleted, once they hold no tabs on the host either.
    fn remove_local_workspace_deletions(&mut self) {
        let base_to_local = match_workspaces(&self.base.workspaces, &self.local.workspaces);
        let base_to_result = match_workspaces(&self.base.workspaces, &self.result.workspaces);
        let mut deleted: Vec<usize> = base_to_local
            .iter()
            .zip(&base_to_result)
            .filter_map(|(local, result)| match (local, result) {
                (None, Some(result)) => Some(*result),
                _ => None,
            })
            .filter(|index| self.result.workspaces[*index].tabs.is_empty())
            .collect();
        deleted.sort_unstable();
        for index in deleted.into_iter().rev() {
            if self.result.workspaces.len() <= 1 {
                break;
            }
            self.result.workspaces.remove(index);
            let active = &mut self.result.active_workspace;
            if *active > index || *active >= self.result.workspaces.len() {
                *active = active.saturating_sub(1);
            }
        }
    }

    fn add_local_tabs(&mut self) {
        let local = self.local;
        let local_to_base = match_workspaces(&local.workspaces, &self.base.workspaces);
        let local_to_result = match_workspaces(&local.workspaces, &self.result.workspaces);
        for (index, workspace) in local.workspaces.iter().enumerate() {
            let has_additions = workspace.session_ids().any(|id| self.added(id));
            let target = match local_to_result[index] {
                Some(target) => target,
                // The host deleted it and the view added nothing to it.
                None if local_to_base[index].is_some() && !has_additions => continue,
                None => {
                    self.result.workspaces.push(StoredWorkspace {
                        active_tab: 0,
                        tabs: Vec::new(),
                        ..workspace.clone()
                    });
                    self.result.workspaces.len() - 1
                }
            };
            for (position, tab) in workspace.tabs.iter().enumerate() {
                self.add_local_tab(target, position, tab);
            }
        }
    }

    fn add_local_tab(&mut self, workspace: usize, position: usize, tab: &StoredTab) {
        if !tab.session_ids().any(|id| self.added(id)) {
            return;
        }
        let shown = tab.session_ids().find_map(|id| find_tab(&self.result, id));
        let mut added_only = tab.clone();
        let removed: Vec<String> = tab
            .session_ids()
            .filter(|id| !self.added(id))
            .map(str::to_owned)
            .collect();
        for id in &removed {
            added_only.remove_pane_or_retile(id);
        }
        match shown {
            Some((shown_workspace, shown_tab)) => {
                let target = &mut self.result.workspaces[shown_workspace].tabs[shown_tab];
                let local_ids: HashSet<&str> = tab.session_ids().collect();
                if target.session_ids().all(|id| local_ids.contains(id)) {
                    // Only the view split this tab: adopt its structure.
                    target.panes = tab.panes.clone();
                    target.layout_tree_json = tab.layout_tree_json.clone();
                    target.active_pane = tab.active_pane;
                    target.zoomed = tab.zoomed;
                } else {
                    // Both sides split it. Keep the host's tree and give the
                    // view's new panes a tab of their own beside it.
                    insert_tab(
                        &mut self.result.workspaces[shown_workspace],
                        shown_tab + 1,
                        added_only,
                    );
                }
            }
            None => insert_tab(&mut self.result.workspaces[workspace], position, added_only),
        }
    }

    /// Field edits the view made since `base`, applied to the matching
    /// workspaces and tabs of the result.
    fn apply_local_edits(&mut self) {
        let (base, local) = (self.base, self.local);
        let local_to_base = match_workspaces(&local.workspaces, &base.workspaces);
        let local_to_result = match_workspaces(&local.workspaces, &self.result.workspaces);
        for (index, workspace) in local.workspaces.iter().enumerate() {
            let (Some(base_index), Some(result_index)) =
                (local_to_base[index], local_to_result[index])
            else {
                continue;
            };
            let before = &base.workspaces[base_index];
            let target = &mut self.result.workspaces[result_index];
            if workspace.name != before.name {
                target.name = workspace.name.clone();
            }
            if workspace.pinned != before.pinned {
                target.pinned = workspace.pinned;
            }
            for tab in &workspace.tabs {
                let Some(before) = tab
                    .session_ids()
                    .find_map(|id| before.tabs.iter().find(|tab| tab.contains_pane(id)))
                else {
                    continue;
                };
                if let Some(target) = tab
                    .session_ids()
                    .find_map(|id| target.tabs.iter().position(|tab| tab.contains_pane(id)))
                    .map(|position| &mut target.tabs[position])
                {
                    apply_tab_edits(before, tab, target);
                }
            }
            let active = workspace.active_tab_identity(None);
            if active != before.active_tab_identity(None)
                && let Some(position) =
                    active.and_then(|id| target.tabs.iter().position(|tab| tab.contains_pane(&id)))
            {
                target.active_tab = position;
            }
        }
        if local.zoom_steps != base.zoom_steps {
            self.result.zoom_steps = local.zoom_steps;
        }
        let active = local_to_base.get(local.active_workspace).copied().flatten();
        if active != Some(base.active_workspace)
            && let Some(Some(target)) = local_to_result.get(local.active_workspace)
        {
            self.result.active_workspace = *target;
        }
    }
}

fn apply_tab_edits(base: &StoredTab, local: &StoredTab, target: &mut StoredTab) {
    if local.pinned != base.pinned {
        target.pinned = local.pinned;
    }
    if local.manual_title != base.manual_title {
        target.manual_title = local.manual_title.clone();
    }
    if local.presentation != base.presentation {
        target.presentation = local.presentation.clone();
    }
    if local.zoomed != base.zoomed {
        target.zoomed = local.zoomed;
    }
    let active_id = |tab: &StoredTab| {
        tab.panes
            .get(tab.active_pane)
            .and_then(|pane| pane.session_id.clone())
    };
    let ids = |tab: &StoredTab| tab.session_ids().map(str::to_owned).collect::<HashSet<_>>();
    let target_active = active_id(target);
    if ids(local) == ids(base)
        && ids(base) == ids(target)
        && (&local.panes, &local.layout_tree_json) != (&base.panes, &base.layout_tree_json)
    {
        // Geometry and the split tree only ever change together.
        target.panes = local.panes.clone();
        target.layout_tree_json = local.layout_tree_json.clone();
    }
    let local_active = active_id(local);
    let active = if local_active != active_id(base) {
        local_active
    } else {
        target_active
    };
    if let Some(position) = target
        .panes
        .iter()
        .position(|pane| active.is_some() && pane.session_id == active)
    {
        target.active_pane = position;
    }
}

fn find_tab(session: &StoredSession, id: &str) -> Option<(usize, usize)> {
    session
        .workspaces
        .iter()
        .enumerate()
        .find_map(|(workspace_index, workspace)| {
            workspace
                .tabs
                .iter()
                .position(|tab| tab.contains_pane(id))
                .map(|tab_index| (workspace_index, tab_index))
        })
}

/// Inserts `tab` at `position` (clamped), keeping the active tab selected.
fn insert_tab(workspace: &mut StoredWorkspace, position: usize, tab: StoredTab) {
    if tab.panes.is_empty() {
        return;
    }
    let position = position.min(workspace.tabs.len());
    if !workspace.tabs.is_empty() && position <= workspace.active_tab {
        workspace.active_tab += 1;
    }
    workspace.tabs.insert(position, tab);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, left: u16, width: u16) -> StoredPane {
        StoredPane {
            session_id: Some(id.into()),
            left,
            top: 0,
            width,
            height: 24,
            buffer: None,
            zoom_steps: 0,
        }
    }

    fn tab(ids: &[&str]) -> StoredTab {
        let width = 80 / ids.len() as u16;
        StoredTab {
            zoomed: false,
            presentation: None,
            pinned: false,
            manual_title: None,
            active_pane: 0,
            layout_tree_json: None,
            panes: ids
                .iter()
                .enumerate()
                .map(|(index, id)| pane(id, index as u16 * width, width))
                .collect(),
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

    fn session(workspaces: Vec<StoredWorkspace>) -> StoredSession {
        StoredSession {
            workspaces,
            active_workspace: 0,
            zoom_steps: 0,
        }
    }

    fn ids(workspace: &StoredWorkspace) -> Vec<Vec<&str>> {
        workspace
            .tabs
            .iter()
            .map(|tab| tab.session_ids().collect())
            .collect()
    }

    /// `termy mux create` added a tab while the view opened its own tab.
    #[test]
    fn host_and_local_new_tabs_are_both_kept() {
        let base = session(vec![workspace("Work", vec![tab(&["a"])])]);
        let mut local = base.clone();
        local.workspaces[0].tabs.push(tab(&["x"]));
        local.workspaces[0].active_tab = 1;
        local.workspaces[0].tabs[0].manual_title = Some("server".into());
        let mut remote = base.clone();
        remote.workspaces[0].tabs.push(tab(&["n"]));
        remote.workspaces[0].active_tab = 1;
        assert!(merge_session(&base, &local, &remote).is_err());
        let merged = rebase_session(&base, &local, &remote);
        assert_eq!(
            ids(&merged.workspaces[0]),
            [vec!["a"], vec!["x"], vec!["n"]]
        );
        assert_eq!(
            merged.workspaces[0].active_tab, 1,
            "the view's selection wins"
        );
        assert_eq!(
            merged.workspaces[0].tabs[0].manual_title.as_deref(),
            Some("server")
        );
        // Rebasing again on the published result changes nothing.
        assert_eq!(rebase_session(&local, &local, &merged), merged);
    }

    /// `termy mux split` on a tab, then a desktop change elsewhere.
    #[test]
    fn host_splits_survive_a_local_new_tab() {
        let base = session(vec![workspace("Work", vec![tab(&["a"])])]);
        let mut local = base.clone();
        local.workspaces[0].tabs.push(tab(&["x"]));
        let mut remote = base.clone();
        remote.workspaces[0].tabs[0] = tab(&["a", "n"]);
        remote.workspaces[0].tabs[0].active_pane = 1;
        let merged = rebase_session(&base, &local, &remote);
        assert_eq!(ids(&merged.workspaces[0]), [vec!["a", "n"], vec!["x"]]);
        assert_eq!(merged.workspaces[0].tabs[0].active_pane, 1);
        assert_eq!(merged.workspaces[0].tabs[0].panes[1].left, 40);
    }

    /// `termy mux close` removed a pane while the view pinned a tab.
    #[test]
    fn host_closures_survive_local_edits() {
        let base = session(vec![workspace("Work", vec![tab(&["a"]), tab(&["b", "c"])])]);
        let mut local = base.clone();
        local.workspaces[0].tabs[0].pinned = true;
        local.workspaces[0].tabs.push(tab(&["x"]));
        let mut remote = base.clone();
        remote.remove_pane("c");
        remote.remove_pane("a");
        let merged = rebase_session(&base, &local, &remote);
        assert_eq!(ids(&merged.workspaces[0]), [vec!["b"], vec!["x"]]);
        assert_eq!(merged.workspaces[0].tabs[0].panes[0].width, 80);
    }

    #[test]
    fn local_closures_survive_host_additions() {
        let base = session(vec![workspace("Work", vec![tab(&["a", "c"]), tab(&["b"])])]);
        let mut local = base.clone();
        local.remove_pane("c");
        local.remove_pane("b");
        local.workspaces[0].tabs[0].manual_title = Some("solo".into());
        let mut remote = base.clone();
        remote.workspaces[0].tabs.push(tab(&["n"]));
        remote.workspaces[0].pinned = true;
        let merged = rebase_session(&base, &local, &remote);
        assert_eq!(ids(&merged.workspaces[0]), [vec!["a"], vec!["n"]]);
        assert_eq!(
            merged.workspaces[0].tabs[0].manual_title.as_deref(),
            Some("solo")
        );
        assert!(merged.workspaces[0].pinned);
    }

    /// Both sides split the same tab: the host's tree wins and the view's new
    /// pane gets a tab of its own instead of being lost.
    #[test]
    fn concurrent_splits_of_one_tab_keep_every_pane() {
        let base = session(vec![workspace("Work", vec![tab(&["a"])])]);
        let mut local = base.clone();
        local.workspaces[0].tabs[0] = tab(&["a", "y"]);
        let mut remote = base.clone();
        remote.workspaces[0].tabs[0] = tab(&["a", "n"]);
        let merged = rebase_session(&base, &local, &remote);
        assert_eq!(ids(&merged.workspaces[0]), [vec!["a", "n"], vec!["y"]]);
        assert_eq!(merged.workspaces[0].tabs[1].panes[0].width, 80);

        // Only the view split it: the view's structure is adopted.
        let mut remote = base.clone();
        remote.workspaces[0].tabs.push(tab(&["n"]));
        let merged = rebase_session(&base, &local, &remote);
        assert_eq!(ids(&merged.workspaces[0]), [vec!["a", "y"], vec!["n"]]);
    }

    #[test]
    fn workspaces_created_and_deleted_on_either_side() {
        let base = session(vec![
            workspace("Work", vec![tab(&["a"])]),
            workspace("Old", Vec::new()),
        ]);
        let mut local = base.clone();
        local.workspaces.remove(1);
        local.workspaces.push(workspace("Local", vec![tab(&["x"])]));
        local.active_workspace = 1;
        let mut remote = base.clone();
        remote.workspaces.push(workspace("Review", Vec::new()));
        remote.workspaces[0].tabs.push(tab(&["n"]));
        let merged = rebase_session(&base, &local, &remote);
        let names: Vec<_> = merged.workspaces.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["Work", "Review", "Local"]);
        assert_eq!(merged.active_workspace, 2, "the view's selection wins");
        assert_eq!(ids(&merged.workspaces[0]), [vec!["a"], vec!["n"]]);
        assert_eq!(ids(&merged.workspaces[2]), [vec!["x"]]);
    }

    #[test]
    fn conflicting_field_edits_prefer_the_view() {
        let base = session(vec![workspace("Work", vec![tab(&["a"])])]);
        let mut local = base.clone();
        local.workspaces[0].name = "Local".into();
        let mut remote = base.clone();
        remote.workspaces[0].name = "Remote".into();
        remote.workspaces[0].pinned = true;
        let merged = rebase_session(&base, &local, &remote);
        assert_eq!(merged.workspaces[0].name, "Local");
        assert!(merged.workspaces[0].pinned);
    }

    #[test]
    fn workspaces_match_by_panes_then_names_then_position() {
        let from = [
            workspace("A", vec![tab(&["a"])]),
            workspace("B", Vec::new()),
            workspace("", Vec::new()),
        ];
        let to = [
            workspace("", Vec::new()),
            workspace("B", Vec::new()),
            workspace("Renamed", vec![tab(&["a", "z"])]),
        ];
        assert_eq!(match_workspaces(&from, &to), [Some(2), Some(1), None]);
        assert_eq!(
            match_workspaces(&from[2..], &to[..1]),
            [Some(0)],
            "equal lengths pair unnamed workspaces by position"
        );
        let renamed = [workspace("Local", vec![tab(&["x"])])];
        assert_eq!(match_workspaces(&from[1..2], &renamed), [None]);
    }
}
