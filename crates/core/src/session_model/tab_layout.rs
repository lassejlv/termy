use crate::session_model::*;
use std::collections::{HashMap, HashSet};

impl StoredTab {
    pub fn layout_bounds(&self) -> NativePaneRect {
        NativePaneRect {
            left: 0,
            top: 0,
            width: self
                .panes
                .iter()
                .map(|pane| pane.left.saturating_add(pane.width))
                .max()
                .unwrap_or(1)
                .max(1),
            height: self
                .panes
                .iter()
                .map(|pane| pane.top.saturating_add(pane.height))
                .max()
                .unwrap_or(1)
                .max(1),
        }
    }
    pub fn layout_tree(&self) -> Result<NativePaneLayoutNode, String> {
        let panes: Vec<_> = self
            .panes
            .iter()
            .map(|pane| {
                Ok(NativePaneInfo {
                    id: pane
                        .session_id
                        .clone()
                        .ok_or("Pane has no live session ID")?,
                    left: pane.left,
                    top: pane.top,
                    width: pane.width,
                    height: pane.height,
                })
            })
            .collect::<Result<_, String>>()?;
        if panes.is_empty() {
            return Err("Tab has no panes".into());
        }
        let ids: Vec<_> = panes.iter().map(|pane| pane.id.clone()).collect();
        if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
            return Err("Tab contains duplicate pane IDs".into());
        }
        let tree = if let Some(json) = &self.layout_tree_json {
            let value = serde_json::from_str(json).map_err(|error| error.to_string())?;
            let stored = NativeLayout::parse_persisted_layout_tree_value(&value)?;
            NativeLayout::native_layout_tree_from_persisted(&stored, &ids)
                .ok_or("Split tree references a missing pane")?
        } else {
            NativeLayout::native_infer_layout_tree_from_rects(
                &panes.iter().collect::<Vec<_>>(),
                self.layout_bounds(),
            )
            .ok_or("Cannot infer the pane split layout")?
        };
        if NativeLayout::native_tree_leaf_count(&tree) != ids.len()
            || !ids
                .iter()
                .all(|id| NativeLayout::native_tree_contains_leaf(&tree, id))
        {
            return Err("Split tree does not match the tab's panes".into());
        }
        Ok(tree)
    }
    pub fn apply_layout_tree(
        &mut self,
        tree: &NativePaneLayoutNode,
        bounds: NativePaneRect,
    ) -> Result<(), String> {
        let mut rects = HashMap::new();
        NativeLayout::native_collect_leaf_rects(tree, bounds, &mut rects);
        let indices: HashMap<_, _> = self
            .panes
            .iter()
            .enumerate()
            .filter_map(|(index, pane)| pane.session_id.as_ref().map(|id| (id.clone(), index)))
            .collect();
        let saved = NativeLayout::persisted_layout_tree_from_native(tree, &indices)
            .ok_or("Split tree contains an unknown pane")?;
        let json = serde_json::to_string(&NativeLayout::persisted_layout_tree_to_value(saved))
            .map_err(|error| error.to_string())?;
        for pane in &mut self.panes {
            let rect = rects
                .get(pane.session_id.as_ref().ok_or("Pane has no session ID")?)
                .ok_or("Pane is absent from split tree")?;
            pane.left = rect.left;
            pane.top = rect.top;
            pane.width = rect.width;
            pane.height = rect.height;
        }
        self.layout_tree_json = Some(json);
        Ok(())
    }
    pub fn split_pane(
        &mut self,
        id: &str,
        new_pane: StoredPane,
        axis: PaneResizeAxis,
    ) -> Result<(), String> {
        let target = self
            .panes
            .iter()
            .find(|pane| pane.session_id.as_deref() == Some(id))
            .ok_or("Pane does not exist")?;
        let extent = match axis {
            PaneResizeAxis::Horizontal => target.width,
            PaneResizeAxis::Vertical => target.height,
        };
        if extent < 2 {
            return Err("Pane is too small to split".into());
        }
        let new_id = new_pane
            .session_id
            .as_ref()
            .ok_or("New pane has no session ID")?;
        if self
            .panes
            .iter()
            .any(|pane| pane.session_id.as_ref() == Some(new_id))
        {
            return Err("Pane ID already exists".into());
        }
        let bounds = self.layout_bounds();
        let mut tree = self.layout_tree()?;
        if !NativeLayout::native_replace_leaf_with_split(&mut tree, id, axis, new_id) {
            return Err("Pane is missing from the split tree".into());
        }
        let mut replacement = self.clone();
        replacement.active_pane = replacement.panes.len();
        replacement.panes.push(new_pane);
        replacement.zoomed = false;
        replacement.apply_layout_tree(&tree, bounds)?;
        *self = replacement;
        Ok(())
    }
    pub fn remove_pane(&mut self, id: &str) -> Result<bool, String> {
        if !self
            .panes
            .iter()
            .any(|pane| pane.session_id.as_deref() == Some(id))
        {
            return Ok(false);
        }
        let bounds = self.layout_bounds();
        let active_id = self
            .panes
            .get(self.active_pane)
            .and_then(|pane| pane.session_id.clone());
        let (tree, focus, _) = NativeLayout::native_remove_leaf_from_tree(self.layout_tree()?, id);
        let mut replacement = self.clone();
        replacement
            .panes
            .retain(|pane| pane.session_id.as_deref() != Some(id));
        let active_id = active_id.filter(|active| active != id).or(focus);
        replacement.active_pane = replacement
            .panes
            .iter()
            .position(|pane| pane.session_id == active_id)
            .unwrap_or(0);
        if let Some(tree) = tree {
            replacement.apply_layout_tree(&tree, bounds)?;
        } else {
            replacement.layout_tree_json = None;
        }
        *self = replacement;
        Ok(true)
    }
}
