use crate::session_model::layout::*;

impl NativeLayout {
    pub fn native_replace_leaf_with_split(
        node: &mut NativePaneLayoutNode,
        target_pane_id: &str,
        axis: PaneResizeAxis,
        new_pane_id: &str,
    ) -> bool {
        Self::native_replace_leaf_with_split_ordered(node, target_pane_id, axis, new_pane_id, false)
    }

    /// Replace the `target_pane_id` leaf with an even split between it and a
    /// new leaf. `new_first` places the new leaf on the left/top side.
    pub fn native_replace_leaf_with_split_ordered(
        node: &mut NativePaneLayoutNode,
        target_pane_id: &str,
        axis: PaneResizeAxis,
        new_pane_id: &str,
        new_first: bool,
    ) -> bool {
        if Self::native_tree_contains_leaf(node, new_pane_id) {
            return false;
        }

        match node {
            NativePaneLayoutNode::Leaf { pane_id } if pane_id == target_pane_id => {
                let existing = NativePaneLayoutNode::Leaf {
                    pane_id: pane_id.clone(),
                };
                let added = NativePaneLayoutNode::Leaf {
                    pane_id: new_pane_id.to_string(),
                };
                let (first, second) = if new_first {
                    (added, existing)
                } else {
                    (existing, added)
                };
                *node = NativePaneLayoutNode::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            NativePaneLayoutNode::Leaf { .. } => false,
            NativePaneLayoutNode::Split { first, second, .. } => {
                Self::native_replace_leaf_with_split_ordered(
                    first,
                    target_pane_id,
                    axis,
                    new_pane_id,
                    new_first,
                ) || Self::native_replace_leaf_with_split_ordered(
                    second,
                    target_pane_id,
                    axis,
                    new_pane_id,
                    new_first,
                )
            }
        }
    }

    /// Swap two leaves in the layout tree by renaming their pane ids.
    /// Returns `true` only when both leaves were found.
    pub fn native_swap_leaves(
        node: &mut NativePaneLayoutNode,
        first_id: &str,
        second_id: &str,
    ) -> bool {
        pub fn walk(
            node: &mut NativePaneLayoutNode,
            first_id: &str,
            second_id: &str,
        ) -> (bool, bool) {
            match node {
                NativePaneLayoutNode::Leaf { pane_id } => {
                    if pane_id == first_id {
                        *pane_id = second_id.to_string();
                        (true, false)
                    } else if pane_id == second_id {
                        *pane_id = first_id.to_string();
                        (false, true)
                    } else {
                        (false, false)
                    }
                }
                NativePaneLayoutNode::Split { first, second, .. } => {
                    let left = walk(first, first_id, second_id);
                    let right = walk(second, first_id, second_id);
                    (left.0 || right.0, left.1 || right.1)
                }
            }
        }

        let (found_first, found_second) = walk(node, first_id, second_id);
        found_first && found_second
    }

    pub fn native_adjust_tree_split(
        node: &mut NativePaneLayoutNode,
        pane_id: &str,
        axis: PaneResizeAxis,
        edge: PaneResizeEdge,
        divider_delta: i16,
        rect: NativePaneRect,
        min_extent: u16,
    ) -> PaneResizeResult {
        match node {
            NativePaneLayoutNode::Leaf { .. } => PaneResizeResult::NoChange,
            NativePaneLayoutNode::Split {
                axis: split_axis,
                ratio,
                first,
                second,
            } => {
                let (first_rect, second_rect) = Self::native_split_rects(*split_axis, *ratio, rect);
                let first_leaf_rect = Self::native_leaf_rect(first, pane_id, first_rect);
                let second_leaf_rect = Self::native_leaf_rect(second, pane_id, second_rect);

                if *split_axis == axis {
                    let total = Self::native_split_extent(axis, rect).max(1);
                    let first_extent = Self::native_split_extent(axis, first_rect);
                    let touches_boundary = match axis {
                        PaneResizeAxis::Horizontal => {
                            (edge == PaneResizeEdge::Right
                                && first_leaf_rect
                                    .is_some_and(|leaf| leaf.right() == first_rect.right()))
                                || (edge == PaneResizeEdge::Left
                                    && second_leaf_rect
                                        .is_some_and(|leaf| leaf.left == second_rect.left))
                        }
                        PaneResizeAxis::Vertical => {
                            (edge == PaneResizeEdge::Bottom
                                && first_leaf_rect
                                    .is_some_and(|leaf| leaf.bottom() == first_rect.bottom()))
                                || (edge == PaneResizeEdge::Top
                                    && second_leaf_rect
                                        .is_some_and(|leaf| leaf.top == second_rect.top))
                        }
                    };

                    if touches_boundary {
                        let next_first_extent = i32::from(first_extent) + i32::from(divider_delta);
                        let next_second_extent = i32::from(total) - next_first_extent;
                        if next_first_extent < i32::from(min_extent)
                            || next_second_extent < i32::from(min_extent)
                        {
                            return PaneResizeResult::BlockedByMinimum;
                        }
                        *ratio = (next_first_extent as f32 / f32::from(total)).clamp(0.0, 1.0);
                        return PaneResizeResult::Applied;
                    }
                }

                let first_result = Self::native_adjust_tree_split(
                    first,
                    pane_id,
                    axis,
                    edge,
                    divider_delta,
                    first_rect,
                    min_extent,
                );
                if first_result != PaneResizeResult::NoChange {
                    return first_result;
                }
                Self::native_adjust_tree_split(
                    second,
                    pane_id,
                    axis,
                    edge,
                    divider_delta,
                    second_rect,
                    min_extent,
                )
            }
        }
    }

    pub fn native_remove_leaf_from_tree(
        node: NativePaneLayoutNode,
        pane_id: &str,
    ) -> (Option<NativePaneLayoutNode>, Option<String>, bool) {
        match node {
            NativePaneLayoutNode::Leaf { pane_id: leaf_id } => {
                if leaf_id == pane_id {
                    (None, None, true)
                } else {
                    (
                        Some(NativePaneLayoutNode::Leaf { pane_id: leaf_id }),
                        None,
                        false,
                    )
                }
            }
            NativePaneLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let original_first = *first;
                let original_second = *second;
                let (next_first, first_focus, removed) =
                    Self::native_remove_leaf_from_tree(original_first.clone(), pane_id);
                if removed {
                    return if let Some(next_first) = next_first {
                        (
                            Some(NativePaneLayoutNode::Split {
                                axis,
                                ratio,
                                first: Box::new(next_first),
                                second: Box::new(original_second),
                            }),
                            first_focus,
                            true,
                        )
                    } else {
                        let focus_id = first_focus
                            .or_else(|| Self::native_tree_first_leaf_id(&original_second));
                        (Some(original_second), focus_id, true)
                    };
                }

                let (next_second, second_focus, removed) =
                    Self::native_remove_leaf_from_tree(original_second.clone(), pane_id);
                if removed {
                    return if let Some(next_second) = next_second {
                        (
                            Some(NativePaneLayoutNode::Split {
                                axis,
                                ratio,
                                first: Box::new(original_first),
                                second: Box::new(next_second),
                            }),
                            second_focus,
                            true,
                        )
                    } else {
                        let focus_id = second_focus
                            .or_else(|| Self::native_tree_first_leaf_id(&original_first));
                        (Some(original_first), focus_id, true)
                    };
                }

                (
                    Some(NativePaneLayoutNode::Split {
                        axis,
                        ratio,
                        first: Box::new(original_first),
                        second: Box::new(original_second),
                    }),
                    None,
                    false,
                )
            }
        }
    }
}
