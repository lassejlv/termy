use crate::session_model::*;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TabEdit {
    Rename {
        title: Option<String>,
    },
    SetPinned {
        pinned: bool,
    },
    SetZoomed {
        zoomed: bool,
    },
    FocusPane {
        id: String,
    },
    ResizeDivider {
        id: String,
        edge: PaneResizeEdge,
        delta: i16,
    },
}

impl TabEdit {
    pub fn apply(&self, tab: &mut StoredTab) -> Result<(), String> {
        match self {
            Self::Rename { title } => {
                if title.as_ref().is_some_and(|title| {
                    title.trim().is_empty()
                        || title.len() > 256
                        || title.chars().any(char::is_control)
                }) {
                    return Err(
                        "Tab title must contain 1 to 256 bytes without control characters".into(),
                    );
                }
                tab.manual_title = title.as_ref().map(|title| title.trim().to_owned());
            }
            Self::SetPinned { pinned } => tab.pinned = *pinned,
            Self::SetZoomed { zoomed } => tab.zoomed = *zoomed,
            Self::FocusPane { id } => {
                tab.active_pane = tab
                    .panes
                    .iter()
                    .position(|pane| pane.session_id.as_deref() == Some(id))
                    .ok_or("Pane is not in this tab")?;
            }
            Self::ResizeDivider { id, edge, delta } => {
                if !tab
                    .panes
                    .iter()
                    .any(|pane| pane.session_id.as_deref() == Some(id))
                {
                    return Err("Pane is not in this tab".into());
                }
                let bounds = tab.layout_bounds();
                let axis = match edge {
                    PaneResizeEdge::Left | PaneResizeEdge::Right => PaneResizeAxis::Horizontal,
                    _ => PaneResizeAxis::Vertical,
                };
                let mut tree = tab.layout_tree()?;
                let extent = NativeLayout::native_split_extent(axis, bounds);
                let minimum = NativeLayout::native_min_extent_allowed(
                    extent,
                    NativeLayout::native_tree_leaf_count(&tree),
                    NativeLayout::native_pane_min_extent_for_axis(axis),
                );
                match NativeLayout::native_adjust_tree_split(
                    &mut tree, id, axis, *edge, *delta, bounds, minimum,
                ) {
                    PaneResizeResult::Applied => {
                        let mut replacement = tab.clone();
                        replacement.apply_layout_tree(&tree, bounds)?;
                        *tab = replacement;
                    }
                    PaneResizeResult::BlockedByMinimum => {
                        return Err("Divider change would make a pane too small".into());
                    }
                    PaneResizeResult::NoChange => {
                        return Err("No divider exists at that pane edge".into());
                    }
                }
            }
        }
        Ok(())
    }
}
