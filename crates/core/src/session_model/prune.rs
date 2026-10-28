//! Removing panes from a saved layout. The host prunes closed and exited
//! panes with these, and the desktop and CLI use them to repair layouts, so
//! every client collapses split trees and follows selections the same way.
use crate::session_model::*;

impl StoredTab {
    /// Removes pane `id` and collapses the split tree like
    /// [`StoredTab::remove_pane`]. When the tree cannot be inferred, the
    /// survivors are re-tiled evenly side by side instead of failing, so the
    /// result is always a layout later edits can infer.
    pub fn remove_pane_or_retile(&mut self, id: &str) -> bool {
        match self.remove_pane(id) {
            Ok(removed) => removed,
            Err(error) => {
                log::warn!("Re-tiling a saved tab after removing pane {id}: {error}");
                let bounds = self.layout_bounds();
                let active_id = self
                    .panes
                    .get(self.active_pane)
                    .and_then(|pane| pane.session_id.clone())
                    .filter(|active| active != id);
                self.panes
                    .retain(|pane| pane.session_id.as_deref() != Some(id));
                self.active_pane = self
                    .panes
                    .iter()
                    .position(|pane| active_id.is_some() && pane.session_id == active_id)
                    .unwrap_or(0);
                self.zoomed = false;
                self.retile_evenly(bounds);
                true
            }
        }
    }

    /// Lays the panes out left to right in equal columns of `bounds`.
    fn retile_evenly(&mut self, bounds: NativePaneRect) {
        let ids: Option<Vec<String>> = self
            .panes
            .iter()
            .map(|pane| pane.session_id.clone())
            .collect();
        let Some(ids) = ids.filter(|ids| !ids.is_empty()) else {
            self.layout_tree_json = None;
            return;
        };
        let count = ids.len();
        let mut tree = NativePaneLayoutNode::Leaf {
            pane_id: ids[count - 1].clone(),
        };
        for (index, id) in ids.iter().enumerate().rev().skip(1) {
            tree = NativePaneLayoutNode::Split {
                axis: PaneResizeAxis::Horizontal,
                ratio: 1.0 / (count - index) as f32,
                first: Box::new(NativePaneLayoutNode::Leaf {
                    pane_id: id.clone(),
                }),
                second: Box::new(tree),
            };
        }
        if self.apply_layout_tree(&tree, bounds).is_err() {
            self.layout_tree_json = None;
        }
    }

    pub(crate) fn session_ids(&self) -> impl Iterator<Item = &str> {
        self.panes
            .iter()
            .filter_map(|pane| pane.session_id.as_deref())
    }

    pub(crate) fn contains_pane(&self, id: &str) -> bool {
        self.session_ids().any(|pane| pane == id)
    }
}

impl StoredWorkspace {
    /// Removes pane `id` from its tab and drops the tab once it is empty.
    /// The active tab follows its identity; when the active tab itself goes,
    /// its right neighbour (or the new last tab) becomes active.
    pub fn remove_pane(&mut self, id: &str) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.contains_pane(id)) else {
            return false;
        };
        let active_id = self.active_tab_identity(Some(id));
        self.tabs[index].remove_pane_or_retile(id);
        if self.tabs[index].panes.is_empty() {
            self.tabs.remove(index);
        }
        self.active_tab = active_id
            .and_then(|active| self.tabs.iter().position(|tab| tab.contains_pane(&active)))
            .unwrap_or_else(|| self.active_tab.min(self.tabs.len().saturating_sub(1)));
        true
    }

    /// A pane of the active tab other than `except`, which identifies that
    /// tab across edits that reorder or remove tabs.
    pub(crate) fn active_tab_identity(&self, except: Option<&str>) -> Option<String> {
        self.tabs
            .get(self.active_tab)?
            .session_ids()
            .find(|pane| Some(*pane) != except)
            .map(str::to_owned)
    }

    pub(crate) fn session_ids(&self) -> impl Iterator<Item = &str> {
        self.tabs.iter().flat_map(StoredTab::session_ids)
    }
}

impl StoredSession {
    /// Removes pane `id` wherever it appears. Workspaces are kept even when
    /// they become empty: an empty workspace is a valid, named place to open
    /// tabs in later.
    pub fn remove_pane(&mut self, id: &str) -> bool {
        let mut removed = false;
        for workspace in &mut self.workspaces {
            removed |= workspace.remove_pane(id);
        }
        removed
    }

    pub fn session_ids(&self) -> impl Iterator<Item = &str> {
        self.workspaces
            .iter()
            .flat_map(StoredWorkspace::session_ids)
    }
}

impl StoredMultiplexer {
    /// An empty layout of the current version.
    pub fn empty() -> Self {
        Self {
            version: STORED_MULTIPLEXER_VERSION,
            windows: Vec::new(),
        }
    }

    /// Removes pane `id` from every window. Returns whether it was listed.
    pub fn remove_pane(&mut self, id: &str) -> bool {
        let mut removed = false;
        for window in &mut self.windows {
            removed |= window.session.remove_pane(id);
        }
        removed
    }

    pub fn session_ids(&self) -> impl Iterator<Item = &str> {
        self.windows
            .iter()
            .flat_map(|window| window.session.session_ids())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str, left: u16, top: u16, width: u16, height: u16) -> StoredPane {
        StoredPane {
            session_id: Some(id.into()),
            left,
            top,
            width,
            height,
            buffer: None,
            zoom_steps: 0,
        }
    }

    fn tab(panes: Vec<StoredPane>) -> StoredTab {
        StoredTab {
            zoomed: false,
            presentation: None,
            pinned: false,
            manual_title: None,
            active_pane: 0,
            layout_tree_json: None,
            panes,
        }
    }

    fn workspace(tabs: Vec<StoredTab>, active_tab: usize) -> StoredWorkspace {
        StoredWorkspace {
            name: "Work".into(),
            pinned: false,
            active_tab,
            tabs,
        }
    }

    /// Audit B16: removing C from `[A | B over C]` must give B the space and
    /// leave a layout later CLI edits can still infer.
    #[test]
    fn removing_a_pane_collapses_the_split_tree() {
        let mut tab = tab(vec![
            pane("a", 0, 0, 40, 24),
            pane("b", 40, 0, 40, 12),
            pane("c", 40, 12, 40, 12),
        ]);
        tab.active_pane = 2;
        assert!(tab.remove_pane_or_retile("c"));
        assert_eq!(
            tab.panes,
            vec![pane("a", 0, 0, 40, 24), pane("b", 40, 0, 40, 24)]
        );
        assert_eq!(tab.active_pane, 1, "focus moves to the sibling");
        assert!(tab.layout_tree().is_ok());
        assert!(!tab.remove_pane_or_retile("missing"));
    }

    #[test]
    fn a_tab_whose_tree_cannot_be_inferred_is_retiled_evenly() {
        // Overlapping rects cannot form a split tree.
        let mut tab = tab(vec![
            pane("a", 0, 0, 60, 24),
            pane("b", 30, 0, 50, 24),
            pane("c", 0, 0, 80, 24),
        ]);
        tab.active_pane = 1;
        tab.zoomed = true;
        assert!(tab.layout_tree().is_err());
        assert!(tab.remove_pane_or_retile("c"));
        assert_eq!(
            tab.panes,
            vec![pane("a", 0, 0, 40, 24), pane("b", 40, 0, 40, 24)]
        );
        assert_eq!(tab.active_pane, 1);
        assert!(!tab.zoomed);
        assert!(tab.layout_tree().is_ok());
    }

    /// Audit L5: the active tab is followed by identity, not clamped.
    #[test]
    fn removing_tabs_follows_the_active_tab_by_identity() {
        let mut work = workspace(
            vec![
                tab(vec![pane("a", 0, 0, 80, 24)]),
                tab(vec![pane("b", 0, 0, 80, 24)]),
                tab(vec![pane("c", 0, 0, 80, 24)]),
            ],
            1,
        );
        assert!(work.remove_pane("a"));
        assert_eq!(work.tabs.len(), 2);
        assert_eq!(work.active_tab, 0, "B stays active");
        // Closing the active tab selects its right neighbour.
        assert!(work.remove_pane("b"));
        assert_eq!(work.active_tab, 0);
        assert_eq!(work.tabs[0].panes[0].session_id.as_deref(), Some("c"));
        assert!(work.remove_pane("c"));
        assert!(work.tabs.is_empty());
        assert_eq!(work.active_tab, 0);
    }

    #[test]
    fn sessions_keep_workspaces_that_become_empty() {
        let mut session = StoredSession {
            workspaces: vec![
                workspace(vec![tab(vec![pane("a", 0, 0, 80, 24)])], 0),
                workspace(vec![tab(vec![pane("b", 0, 0, 80, 24)])], 0),
            ],
            active_workspace: 1,
            zoom_steps: 0,
        };
        assert!(session.remove_pane("b"));
        assert_eq!(session.workspaces.len(), 2);
        assert!(session.workspaces[1].tabs.is_empty());
        assert_eq!(session.active_workspace, 1);
        assert_eq!(session.session_ids().collect::<Vec<_>>(), ["a"]);
        assert!(!session.remove_pane("b"));
    }
}
