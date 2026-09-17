use super::*;

/// Pointer travel (px) before a modifier-press becomes a pane-move drag, so
/// an Alt+Cmd click without movement stays inert.
const PANE_MOVE_DRAG_THRESHOLD_PX: f32 = 4.0;

/// Fraction of the pane treated as the swap (center) region; outside it the
/// nearest edge wins and the drop becomes a split on that side.
const PANE_MOVE_CENTER_FRACTION: f32 = 0.30;

/// Pane move drop zone: an edge half (insert as a split on that side) or the
/// center (swap).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::terminal_view) enum PaneDropRegion {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal_view) enum PaneMoveDropTarget {
    Pane {
        pane_id: String,
        region: PaneDropRegion,
    },
    Tab {
        tab_id: TabId,
    },
}

#[derive(Clone, Debug)]
pub(in crate::terminal_view) struct PaneMoveHandleDrag {
    pub(in crate::terminal_view) pane_id: String,
}

#[derive(Clone, Debug)]
pub(in crate::terminal_view) struct PaneMoveDragState {
    pub(in crate::terminal_view) pane_id: String,
    pub(in crate::terminal_view) start_x: f32,
    pub(in crate::terminal_view) start_y: f32,
    /// Set once the pointer travels past the drag threshold; the placement
    /// overlay only renders for active drags so a modifier-click stays inert.
    pub(in crate::terminal_view) active: bool,
    pub(in crate::terminal_view) drop_target: Option<PaneMoveDropTarget>,
}

impl TerminalView {
    /// Alt + the platform secondary modifier (Cmd on macOS, Ctrl elsewhere)
    /// starts a pane-move drag. Link clicks use secondary *without* alt and
    /// plain selection uses no modifier, so this combination is free.
    pub(in super::super) fn is_pane_move_modifier(modifiers: gpui::Modifiers) -> bool {
        modifiers.alt && modifiers.secondary()
    }

    pub(in super::super) fn pane_move_drag_active(&self) -> bool {
        self.pane_move_drag.as_ref().is_some_and(|drag| drag.active)
    }

    pub(in super::super) fn try_begin_pane_move_drag(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.runtime_kind() != RuntimeKind::Native {
            return false;
        }
        let multi_pane = self
            .session
            .tabs
            .get(self.session.active_tab)
            .is_some_and(|tab| tab.panes.len() > 1);
        if !multi_pane {
            return false;
        }
        let Some((pane_id, _, _)) = self.pane_move_hit_test(position, window) else {
            return false;
        };

        let (x, y) = self.terminal_content_position(position);
        if !self.is_active_pane_id(pane_id.as_str()) {
            let _ = self.focus_pane_target(pane_id.as_str(), cx);
        }
        self.pane_move_drag = Some(PaneMoveDragState {
            pane_id,
            start_x: x,
            start_y: y,
            active: false,
            drop_target: None,
        });
        true
    }

    /// Start an already-activated GPUI drag from the pane's top-center grab
    /// handle. Clicks without movement are handled separately by the handle.
    pub(in super::super) fn begin_pane_move_drag_from_handle(
        &mut self,
        pane_id: String,
        position: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.runtime_kind() != RuntimeKind::Native {
            return;
        }
        let pane_exists = self
            .session
            .tabs
            .get(self.session.active_tab)
            .is_some_and(|tab| tab.panes.len() > 1 && tab.panes.iter().any(|p| p.id == pane_id));
        if !pane_exists {
            return;
        }

        if !self.is_active_pane_id(pane_id.as_str()) {
            let _ = self.focus_pane_target(pane_id.as_str(), cx);
        }
        let (x, y) = self.terminal_content_position(position);
        let drop_target = self.pane_move_drop_target(position, window, pane_id.as_str());
        self.pane_move_drag = Some(PaneMoveDragState {
            pane_id,
            start_x: x,
            start_y: y,
            active: true,
            drop_target,
        });
        cx.notify();
    }

    pub(in super::super) fn update_pane_move_drag(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.pane_move_drag.clone() else {
            return;
        };

        let (x, y) = self.terminal_content_position(position);
        let active = drag.active
            || (x - drag.start_x).abs() >= PANE_MOVE_DRAG_THRESHOLD_PX
            || (y - drag.start_y).abs() >= PANE_MOVE_DRAG_THRESHOLD_PX;
        let drop_target = if active {
            self.pane_move_drop_target(position, window, drag.pane_id.as_str())
        } else {
            None
        };

        let Some(drag) = self.pane_move_drag.as_mut() else {
            return;
        };
        if drag.active != active || drag.drop_target != drop_target {
            drag.active = active;
            drag.drop_target = drop_target;
            cx.notify();
        }
    }

    /// Commit (when a valid target is highlighted) or cancel the drag.
    /// Returns whether a drag existed at all.
    pub(in super::super) fn finish_pane_move_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.pane_move_drag.take() else {
            return false;
        };
        if drag.active
            && let Some(target) = drag.drop_target
        {
            match target {
                PaneMoveDropTarget::Pane { pane_id, region } => {
                    self.apply_pane_move(drag.pane_id.as_str(), pane_id.as_str(), region, cx);
                }
                PaneMoveDropTarget::Tab { tab_id } => {
                    self.apply_pane_move_to_tab(drag.pane_id.as_str(), tab_id, cx);
                }
            }
        }
        cx.notify();
        true
    }

    /// Pane under the pointer plus the pointer's fractional position within
    /// its frame.
    fn pane_move_hit_test(
        &self,
        position: gpui::Point<Pixels>,
        window: &Window,
    ) -> Option<(String, f32, f32)> {
        let content_bounds = self.terminal_content_bounds(window)?;
        let (x, y) = self.terminal_content_position(position);
        let tab = self.active_tab_ref()?;
        for pane in &tab.panes {
            let Some(layout) = self.terminal_pane_layout(tab, pane, content_bounds) else {
                continue;
            };
            let frame = layout.frame;
            if x < frame.origin_x
                || x >= frame.right()
                || y < frame.origin_y
                || y >= frame.bottom()
                || frame.width <= f32::EPSILON
                || frame.height <= f32::EPSILON
            {
                continue;
            }
            return Some((
                pane.id.clone(),
                (x - frame.origin_x) / frame.width,
                (y - frame.origin_y) / frame.height,
            ));
        }
        None
    }

    fn pane_move_drop_target(
        &self,
        position: gpui::Point<Pixels>,
        window: &Window,
        source_pane_id: &str,
    ) -> Option<PaneMoveDropTarget> {
        if let Some(tab_id) = self.pane_move_tab_drop_target(position, window)
            && self
                .session
                .tabs
                .get(self.session.active_tab)
                .is_none_or(|tab| tab.id != tab_id)
        {
            return Some(PaneMoveDropTarget::Tab { tab_id });
        }
        let (pane_id, fx, fy) = self.pane_move_hit_test(position, window)?;
        if pane_id == source_pane_id {
            return None;
        }
        let region = Self::pane_drop_region_for_fractions(fx, fy);
        if !self.pane_drop_region_is_splittable(pane_id.as_str(), region) {
            return None;
        }
        Some(PaneMoveDropTarget::Pane { pane_id, region })
    }

    fn pane_move_tab_drop_target(
        &self,
        position: gpui::Point<Pixels>,
        window: &Window,
    ) -> Option<TabId> {
        if !self.should_render_tab_strip_chrome() {
            return None;
        }
        let x: f32 = position.x.into();
        let y: f32 = position.y.into();
        let index = match self.tab_strip_orientation() {
            crate::terminal_view::tab_strip::state::TabStripOrientation::Horizontal => {
                let geometry = self.tab_strip_geometry(window);
                if !(TOP_STRIP_CONTENT_OFFSET_Y..TOP_STRIP_CONTENT_OFFSET_Y + TABBAR_HEIGHT)
                    .contains(&y)
                    || !geometry.contains_tabs_viewport_x(x)
                {
                    return None;
                }
                let pointer = x - geometry.row_start_x;
                let scroll: f32 = self.tab_strip.horizontal_scroll_handle.offset().x.into();
                Self::pane_move_tab_index_for_axis(
                    self.session.tabs.iter().map(|tab| tab.display_width),
                    pointer,
                    scroll,
                    TAB_HORIZONTAL_PADDING,
                    TAB_ITEM_GAP,
                )?
            }
            crate::terminal_view::tab_strip::state::TabStripOrientation::Vertical => {
                if self.sidebar_collapsed {
                    return None;
                }
                let viewport_width: f32 = window.viewport_size().width.into();
                if x < viewport_width - SIDEBAR_WIDTH || x >= viewport_width {
                    return None;
                }
                let pointer = y - self.terminal_content_top_inset() - SIDEBAR_HEADER_HEIGHT;
                let scroll: f32 = self.tab_strip.vertical_scroll_handle.offset().y.into();
                Self::pane_move_tab_index_for_axis(
                    std::iter::repeat_n(SIDEBAR_TAB_ROW_HEIGHT, self.session.tabs.len()),
                    pointer,
                    scroll,
                    SIDEBAR_TAB_PADDING_Y,
                    SIDEBAR_TAB_ROW_GAP,
                )?
            }
        };
        self.session.tabs.get(index).and_then(|tab| {
            let active_pane = tab
                .panes
                .iter()
                .find(|pane| pane.id == tab.active_pane_id)?;
            let min_width =
                NativeLayout::native_pane_min_extent_for_axis(PaneResizeAxis::Horizontal);
            (!self
                .session
                .native_pane_zoom_snapshots
                .contains_key(&tab.id)
                && active_pane.width >= min_width.saturating_mul(2))
            .then_some(tab.id)
        })
    }

    fn pane_move_tab_index_for_axis(
        extents: impl IntoIterator<Item = f32>,
        pointer: f32,
        scroll: f32,
        leading_inset: f32,
        gap: f32,
    ) -> Option<usize> {
        let mut start = leading_inset + scroll;
        for (index, extent) in extents.into_iter().enumerate() {
            let end = start + extent;
            if pointer >= start && pointer < end {
                return Some(index);
            }
            start = end + gap;
        }
        None
    }

    pub(in super::super) fn pane_move_targets_tab(&self, tab_id: TabId) -> bool {
        self.pane_move_drag.as_ref().is_some_and(|drag| {
            drag.active
                && matches!(drag.drop_target, Some(PaneMoveDropTarget::Tab { tab_id: target }) if target == tab_id)
        })
    }

    fn pane_drop_region_for_fractions(fx: f32, fy: f32) -> PaneDropRegion {
        let center_margin = (1.0 - PANE_MOVE_CENTER_FRACTION) * 0.5;
        if (center_margin..=1.0 - center_margin).contains(&fx)
            && (center_margin..=1.0 - center_margin).contains(&fy)
        {
            return PaneDropRegion::Center;
        }
        let edge_distances = [
            (fx, PaneDropRegion::Left),
            (1.0 - fx, PaneDropRegion::Right),
            (fy, PaneDropRegion::Top),
            (1.0 - fy, PaneDropRegion::Bottom),
        ];
        edge_distances
            .into_iter()
            .min_by(|(left, _), (right, _)| left.total_cmp(right))
            .map_or(PaneDropRegion::Center, |(_, region)| region)
    }

    /// Edge drops split the target pane in half, which requires the target to
    /// have room for two minimum-size panes. Center (swap) is always valid.
    fn pane_drop_region_is_splittable(&self, target_pane_id: &str, region: PaneDropRegion) -> bool {
        let Some(tab) = self.active_tab_ref() else {
            return false;
        };
        let Some(pane) = tab.panes.iter().find(|pane| pane.id == target_pane_id) else {
            return false;
        };
        match region {
            PaneDropRegion::Center => true,
            PaneDropRegion::Left | PaneDropRegion::Right => {
                let min_width =
                    NativeLayout::native_pane_min_extent_for_axis(PaneResizeAxis::Horizontal);
                pane.width >= min_width.saturating_mul(2)
            }
            PaneDropRegion::Top | PaneDropRegion::Bottom => {
                let min_height =
                    NativeLayout::native_pane_min_extent_for_axis(PaneResizeAxis::Vertical);
                pane.height >= min_height.saturating_mul(2)
            }
        }
    }

    fn apply_pane_move(
        &mut self,
        source_pane_id: &str,
        target_pane_id: &str,
        region: PaneDropRegion,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.runtime_kind() != RuntimeKind::Native || source_pane_id == target_pane_id {
            return false;
        }
        let Some(tab) = self.session.tabs.get(self.session.active_tab) else {
            return false;
        };
        let tab_id = tab.id;
        let has_both_panes = tab.panes.iter().any(|pane| pane.id == source_pane_id)
            && tab.panes.iter().any(|pane| pane.id == target_pane_id);
        if !has_both_panes {
            return false;
        }
        let cols = tab
            .panes
            .iter()
            .map(|pane| pane.left.saturating_add(pane.width))
            .max()
            .unwrap_or(1)
            .max(1);
        let rows = tab
            .panes
            .iter()
            .map(|pane| pane.top.saturating_add(pane.height))
            .max()
            .unwrap_or(1)
            .max(1);

        self.clear_native_zoom_snapshot_for_active_tab();
        if !self.ensure_native_layout_tree_for_tab_id(tab_id) {
            return false;
        }

        let restructured = match region {
            PaneDropRegion::Center => self
                .session
                .native_pane_layout_trees
                .get_mut(&tab_id)
                .is_some_and(|tree| {
                    NativeLayout::native_swap_leaves(&mut tree.root, source_pane_id, target_pane_id)
                }),
            PaneDropRegion::Left
            | PaneDropRegion::Right
            | PaneDropRegion::Top
            | PaneDropRegion::Bottom => {
                let Some(tree) = self.session.native_pane_layout_trees.remove(&tab_id) else {
                    return false;
                };
                let (next_root, _, removed) =
                    NativeLayout::native_remove_leaf_from_tree(tree.root, source_pane_id);
                let Some(mut root) = next_root.filter(|_| removed) else {
                    // The tree is stale; drop it so it is rebuilt from pane
                    // geometry on next use.
                    return false;
                };
                let (axis, source_first) = match region {
                    PaneDropRegion::Left => (PaneResizeAxis::Horizontal, true),
                    PaneDropRegion::Right => (PaneResizeAxis::Horizontal, false),
                    PaneDropRegion::Top => (PaneResizeAxis::Vertical, true),
                    PaneDropRegion::Bottom => (PaneResizeAxis::Vertical, false),
                    PaneDropRegion::Center => unreachable!(),
                };
                if !NativeLayout::native_replace_leaf_with_split_ordered(
                    &mut root,
                    target_pane_id,
                    axis,
                    source_pane_id,
                    source_first,
                ) {
                    return false;
                }
                NativeLayout::native_balance_split_group_containing_leaf(
                    &mut root,
                    axis,
                    source_pane_id,
                );
                self.session
                    .native_pane_layout_trees
                    .insert(tab_id, NativePaneLayoutTree { root });
                true
            }
        };
        if !restructured {
            return false;
        }

        self.apply_native_layout_tree_to_tab(tab_id, cols, rows);
        if let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) {
            // Focus follows the moved pane.
            tab.active_pane_id = source_pane_id.to_string();
            tab.assert_active_pane_invariant();
        }
        self.clear_selection();
        self.clear_hovered_link();
        self.clear_terminal_scrollbar_marker_cache();
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    fn apply_pane_move_to_tab(
        &mut self,
        source_pane_id: &str,
        target_tab_id: TabId,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.runtime_kind() != RuntimeKind::Native {
            return false;
        }
        let source_index = self.session.active_tab;
        let Some(target_index) = self.tab_index_by_id(target_tab_id) else {
            return false;
        };
        if source_index == target_index
            || self
                .session
                .tabs
                .get(source_index)
                .is_none_or(|tab| tab.panes.len() <= 1)
        {
            return false;
        }
        let source_tab_id = self.session.tabs[source_index].id;
        if self
            .session
            .native_pane_zoom_snapshots
            .contains_key(&target_tab_id)
        {
            return false;
        }
        let target_pane_id = self.session.tabs[target_index].active_pane_id.clone();
        let min_width = NativeLayout::native_pane_min_extent_for_axis(PaneResizeAxis::Horizontal);
        if self.session.tabs[target_index]
            .panes
            .iter()
            .find(|pane| pane.id == target_pane_id)
            .is_none_or(|pane| pane.width < min_width.saturating_mul(2))
        {
            return false;
        }
        let Some(source_pane_index) = self.session.tabs[source_index]
            .panes
            .iter()
            .position(|pane| pane.id == source_pane_id)
        else {
            return false;
        };
        let target_cols = self.session.tabs[target_index]
            .panes
            .iter()
            .map(|pane| pane.left.saturating_add(pane.width))
            .max()
            .unwrap_or(1)
            .max(1);
        let target_rows = self.session.tabs[target_index]
            .panes
            .iter()
            .map(|pane| pane.top.saturating_add(pane.height))
            .max()
            .unwrap_or(1)
            .max(1);
        let source_cols = self.session.tabs[source_index]
            .panes
            .iter()
            .map(|pane| pane.left.saturating_add(pane.width))
            .max()
            .unwrap_or(1)
            .max(1);
        let source_rows = self.session.tabs[source_index]
            .panes
            .iter()
            .map(|pane| pane.top.saturating_add(pane.height))
            .max()
            .unwrap_or(1)
            .max(1);

        self.session
            .native_pane_zoom_snapshots
            .remove(&source_tab_id);
        if !self.ensure_native_layout_tree_for_tab_id(source_tab_id)
            || !self.ensure_native_layout_tree_for_tab_id(target_tab_id)
        {
            return false;
        }
        let Some(source_tree) = self.session.native_pane_layout_trees.remove(&source_tab_id) else {
            return false;
        };
        let Some(target_tree) = self.session.native_pane_layout_trees.remove(&target_tab_id) else {
            self.session
                .native_pane_layout_trees
                .insert(source_tab_id, source_tree);
            return false;
        };
        let Some((source_root, target_root, next_focus_id)) = Self::move_leaf_between_tab_trees(
            source_tree.root.clone(),
            target_tree.root.clone(),
            source_pane_id,
            target_pane_id.as_str(),
        ) else {
            self.session
                .native_pane_layout_trees
                .insert(source_tab_id, source_tree);
            self.session
                .native_pane_layout_trees
                .insert(target_tab_id, target_tree);
            return false;
        };
        self.session
            .native_pane_layout_trees
            .insert(source_tab_id, NativePaneLayoutTree { root: source_root });
        self.session
            .native_pane_layout_trees
            .insert(target_tab_id, NativePaneLayoutTree { root: target_root });
        let pane = self.session.tabs[source_index]
            .panes
            .remove(source_pane_index);
        self.session.tabs[source_index].active_pane_id = next_focus_id
            .or_else(|| {
                self.session.tabs[source_index]
                    .panes
                    .first()
                    .map(|pane| pane.id.clone())
            })
            .unwrap_or_default();
        self.session.tabs[source_index].assert_active_pane_invariant();
        self.session.tabs[target_index].panes.push(pane);
        self.session.tabs[target_index].active_pane_id = source_pane_id.to_string();
        self.session.tabs[target_index].assert_active_pane_invariant();
        self.refresh_tab_title(source_index);
        self.refresh_tab_title(target_index);

        self.apply_native_layout_tree_to_tab(source_tab_id, source_cols, source_rows);
        self.apply_native_layout_tree_to_tab(target_tab_id, target_cols, target_rows);
        self.switch_tab(target_index, cx);
        self.clear_selection();
        self.clear_hovered_link();
        self.clear_terminal_scrollbar_marker_cache();
        self.evict_inactive_terminal_render_caches();
        self.sync_native_terminal_wakeup_interest();
        self.schedule_persist_native_workspace(cx);
        cx.notify();
        true
    }

    fn move_leaf_between_tab_trees(
        source_root: NativePaneLayoutNode,
        mut target_root: NativePaneLayoutNode,
        source_pane_id: &str,
        target_pane_id: &str,
    ) -> Option<(NativePaneLayoutNode, NativePaneLayoutNode, Option<String>)> {
        let (source_root, next_focus_id, removed) =
            NativeLayout::native_remove_leaf_from_tree(source_root, source_pane_id);
        let source_root = source_root.filter(|_| removed)?;
        if !NativeLayout::native_replace_leaf_with_split_ordered(
            &mut target_root,
            target_pane_id,
            PaneResizeAxis::Horizontal,
            source_pane_id,
            false,
        ) {
            return None;
        }
        Some((source_root, target_root, next_focus_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_region_center_box_maps_to_swap() {
        assert_eq!(
            TerminalView::pane_drop_region_for_fractions(0.5, 0.5),
            PaneDropRegion::Center
        );
        assert_eq!(
            TerminalView::pane_drop_region_for_fractions(0.4, 0.6),
            PaneDropRegion::Center
        );
    }

    #[test]
    fn drop_region_edges_pick_nearest_side() {
        assert_eq!(
            TerminalView::pane_drop_region_for_fractions(0.05, 0.5),
            PaneDropRegion::Left
        );
        assert_eq!(
            TerminalView::pane_drop_region_for_fractions(0.95, 0.5),
            PaneDropRegion::Right
        );
        assert_eq!(
            TerminalView::pane_drop_region_for_fractions(0.5, 0.05),
            PaneDropRegion::Top
        );
        assert_eq!(
            TerminalView::pane_drop_region_for_fractions(0.5, 0.95),
            PaneDropRegion::Bottom
        );
    }

    #[test]
    fn drop_region_corner_picks_dominant_edge() {
        assert_eq!(
            TerminalView::pane_drop_region_for_fractions(0.02, 0.2),
            PaneDropRegion::Left
        );
        assert_eq!(
            TerminalView::pane_drop_region_for_fractions(0.2, 0.02),
            PaneDropRegion::Top
        );
    }

    #[test]
    fn pane_move_tab_hit_test_ignores_gaps() {
        let extents = [100.0, 80.0];
        assert_eq!(
            TerminalView::pane_move_tab_index_for_axis(extents, 55.0, 0.0, 6.0, 4.0),
            Some(0)
        );
        assert_eq!(
            TerminalView::pane_move_tab_index_for_axis(extents, 107.0, 0.0, 6.0, 4.0),
            None
        );
        assert_eq!(
            TerminalView::pane_move_tab_index_for_axis(extents, 120.0, 0.0, 6.0, 4.0),
            Some(1)
        );
    }

    #[test]
    fn moving_leaf_between_tabs_removes_source_and_splits_target() {
        let source = NativePaneLayoutNode::Split {
            axis: PaneResizeAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(NativePaneLayoutNode::Leaf {
                pane_id: "source".to_string(),
            }),
            second: Box::new(NativePaneLayoutNode::Leaf {
                pane_id: "remaining".to_string(),
            }),
        };
        let target = NativePaneLayoutNode::Leaf {
            pane_id: "target".to_string(),
        };

        let (source, target, next_focus) =
            TerminalView::move_leaf_between_tab_trees(source, target, "source", "target")
                .expect("pane should move between valid tab trees");

        assert!(matches!(
            source,
            NativePaneLayoutNode::Leaf { pane_id } if pane_id == "remaining"
        ));
        assert_eq!(next_focus.as_deref(), Some("remaining"));
        assert!(matches!(
            target,
            NativePaneLayoutNode::Split { first, second, .. }
                if matches!(*first, NativePaneLayoutNode::Leaf { ref pane_id } if pane_id == "target")
                    && matches!(*second, NativePaneLayoutNode::Leaf { ref pane_id } if pane_id == "source")
        ));
    }

    #[test]
    fn swap_leaves_renames_both_sides() {
        let mut root = NativePaneLayoutNode::Split {
            axis: PaneResizeAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(NativePaneLayoutNode::Leaf {
                pane_id: "a".to_string(),
            }),
            second: Box::new(NativePaneLayoutNode::Split {
                axis: PaneResizeAxis::Vertical,
                ratio: 0.5,
                first: Box::new(NativePaneLayoutNode::Leaf {
                    pane_id: "b".to_string(),
                }),
                second: Box::new(NativePaneLayoutNode::Leaf {
                    pane_id: "c".to_string(),
                }),
            }),
        };
        assert!(NativeLayout::native_swap_leaves(&mut root, "a", "c"));
        match &root {
            NativePaneLayoutNode::Split { first, second, .. } => {
                assert!(
                    matches!(&**first, NativePaneLayoutNode::Leaf { pane_id } if pane_id == "c")
                );
                match &**second {
                    NativePaneLayoutNode::Split { second, .. } => {
                        assert!(matches!(
                            &**second,
                            NativePaneLayoutNode::Leaf { pane_id } if pane_id == "a"
                        ));
                    }
                    _ => panic!("expected nested split"),
                }
            }
            _ => panic!("expected split root"),
        }
    }

    #[test]
    fn swap_leaves_requires_both_leaves_present() {
        let mut root = NativePaneLayoutNode::Leaf {
            pane_id: "a".to_string(),
        };
        assert!(!NativeLayout::native_swap_leaves(&mut root, "a", "missing"));
    }

    #[test]
    fn ordered_split_places_new_leaf_first_or_second() {
        let mut root = NativePaneLayoutNode::Leaf {
            pane_id: "target".to_string(),
        };
        assert!(NativeLayout::native_replace_leaf_with_split_ordered(
            &mut root,
            "target",
            PaneResizeAxis::Horizontal,
            "new",
            true,
        ));
        match &root {
            NativePaneLayoutNode::Split { first, second, .. } => {
                assert!(
                    matches!(&**first, NativePaneLayoutNode::Leaf { pane_id } if pane_id == "new")
                );
                assert!(matches!(
                    &**second,
                    NativePaneLayoutNode::Leaf { pane_id } if pane_id == "target"
                ));
            }
            _ => panic!("expected split"),
        }
    }
}
