//! Desktop split geometry shared by native and CLI hosts.
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativePaneRect {
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativePaneLayoutTree {
    pub root: NativePaneLayoutNode,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativePaneLayoutNode {
    Leaf {
        pane_id: String,
    },
    Split {
        axis: PaneResizeAxis,
        ratio: f32,
        first: Box<NativePaneLayoutNode>,
        second: Box<NativePaneLayoutNode>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneResizeAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneResizeResult {
    Applied,
    BlockedByMinimum,
    NoChange,
}

impl NativePaneRect {
    pub fn right(self) -> u16 {
        self.left.saturating_add(self.width)
    }

    pub fn bottom(self) -> u16 {
        self.top.saturating_add(self.height)
    }
}

pub struct NativeLayout;

impl NativeLayout {
    pub fn native_leaf_rect(
        node: &NativePaneLayoutNode,
        target_pane_id: &str,
        rect: NativePaneRect,
    ) -> Option<NativePaneRect> {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => (pane_id == target_pane_id).then_some(rect),
            NativePaneLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (first_rect, second_rect) = Self::native_split_rects(*axis, *ratio, rect);
                Self::native_leaf_rect(first, target_pane_id, first_rect)
                    .or_else(|| Self::native_leaf_rect(second, target_pane_id, second_rect))
            }
        }
    }

    pub fn native_tree_leaf_count(node: &NativePaneLayoutNode) -> usize {
        match node {
            NativePaneLayoutNode::Leaf { .. } => 1,
            NativePaneLayoutNode::Split { first, second, .. } => {
                Self::native_tree_leaf_count(first) + Self::native_tree_leaf_count(second)
            }
        }
    }

    pub fn native_tree_first_leaf_id(node: &NativePaneLayoutNode) -> Option<String> {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => Some(pane_id.clone()),
            NativePaneLayoutNode::Split { first, .. } => Self::native_tree_first_leaf_id(first),
        }
    }

    pub fn native_tree_contains_leaf(node: &NativePaneLayoutNode, target_pane_id: &str) -> bool {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => pane_id == target_pane_id,
            NativePaneLayoutNode::Split { first, second, .. } => {
                Self::native_tree_contains_leaf(first, target_pane_id)
                    || Self::native_tree_contains_leaf(second, target_pane_id)
            }
        }
    }

    pub fn native_axis_group_contains_leaf(
        node: &NativePaneLayoutNode,
        axis: PaneResizeAxis,
        target_pane_id: &str,
    ) -> bool {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => pane_id == target_pane_id,
            NativePaneLayoutNode::Split {
                axis: split_axis,
                first,
                second,
                ..
            } if *split_axis == axis => {
                Self::native_axis_group_contains_leaf(first, axis, target_pane_id)
                    || Self::native_axis_group_contains_leaf(second, axis, target_pane_id)
            }
            NativePaneLayoutNode::Split { .. } => false,
        }
    }

    pub fn native_collect_axis_group_nodes(
        node: NativePaneLayoutNode,
        axis: PaneResizeAxis,
        nodes: &mut Vec<NativePaneLayoutNode>,
    ) {
        match node {
            NativePaneLayoutNode::Split {
                axis: split_axis,
                first,
                second,
                ..
            } if split_axis == axis => {
                Self::native_collect_axis_group_nodes(*first, axis, nodes);
                Self::native_collect_axis_group_nodes(*second, axis, nodes);
            }
            node => nodes.push(node),
        }
    }

    pub fn native_rebuild_even_axis_group(
        axis: PaneResizeAxis,
        mut nodes: Vec<NativePaneLayoutNode>,
    ) -> Option<NativePaneLayoutNode> {
        if nodes.len() <= 1 {
            return nodes.pop();
        }

        let total_count = nodes.len();
        let split_index = total_count / 2;
        let right_nodes = nodes.split_off(split_index);
        let first = Self::native_rebuild_even_axis_group(axis, nodes)
            .expect("balanced native split group must have a first branch");
        let second = Self::native_rebuild_even_axis_group(axis, right_nodes)
            .expect("balanced native split group must have a second branch");

        Some(NativePaneLayoutNode::Split {
            axis,
            ratio: split_index as f32 / total_count as f32,
            first: Box::new(first),
            second: Box::new(second),
        })
    }

    pub fn native_balance_axis_group(node: &mut NativePaneLayoutNode, axis: PaneResizeAxis) {
        let placeholder = NativePaneLayoutNode::Leaf {
            pane_id: String::new(),
        };
        let original = std::mem::replace(node, placeholder);
        let mut nodes = Vec::new();
        Self::native_collect_axis_group_nodes(original, axis, &mut nodes);
        if let Some(rebuilt) = Self::native_rebuild_even_axis_group(axis, nodes) {
            *node = rebuilt;
        }
    }

    pub fn native_balance_split_group_containing_leaf(
        node: &mut NativePaneLayoutNode,
        axis: PaneResizeAxis,
        pane_id: &str,
    ) -> bool {
        if matches!(
            node,
            NativePaneLayoutNode::Split {
                axis: split_axis,
                ..
            } if *split_axis == axis
        ) && Self::native_axis_group_contains_leaf(node, axis, pane_id)
        {
            Self::native_balance_axis_group(node, axis);
            return true;
        }

        match node {
            NativePaneLayoutNode::Leaf { .. } => false,
            NativePaneLayoutNode::Split { first, second, .. } => {
                Self::native_balance_split_group_containing_leaf(first, axis, pane_id)
                    || Self::native_balance_split_group_containing_leaf(second, axis, pane_id)
            }
        }
    }

    pub fn native_split_extent(axis: PaneResizeAxis, rect: NativePaneRect) -> u16 {
        match axis {
            PaneResizeAxis::Horizontal => rect.width,
            PaneResizeAxis::Vertical => rect.height,
        }
    }

    pub fn native_split_rects(
        axis: PaneResizeAxis,
        ratio: f32,
        rect: NativePaneRect,
    ) -> (NativePaneRect, NativePaneRect) {
        let total = Self::native_split_extent(axis, rect);
        let first_extent = if total <= 1 {
            1
        } else {
            ((f32::from(total) * ratio.clamp(0.0, 1.0)).round() as u16)
                .clamp(1, total.saturating_sub(1))
        };
        match axis {
            PaneResizeAxis::Horizontal => (
                NativePaneRect {
                    width: first_extent,
                    ..rect
                },
                NativePaneRect {
                    left: rect.left.saturating_add(first_extent),
                    width: total.saturating_sub(first_extent).max(1),
                    ..rect
                },
            ),
            PaneResizeAxis::Vertical => (
                NativePaneRect {
                    height: first_extent,
                    ..rect
                },
                NativePaneRect {
                    top: rect.top.saturating_add(first_extent),
                    height: total.saturating_sub(first_extent).max(1),
                    ..rect
                },
            ),
        }
    }

    pub fn native_collect_leaf_rects(
        node: &NativePaneLayoutNode,
        rect: NativePaneRect,
        rects: &mut HashMap<String, NativePaneRect>,
    ) {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => {
                rects.insert(pane_id.clone(), rect);
            }
            NativePaneLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (first_rect, second_rect) = Self::native_split_rects(*axis, *ratio, rect);
                Self::native_collect_leaf_rects(first, first_rect, rects);
                Self::native_collect_leaf_rects(second, second_rect, rects);
            }
        }
    }
}

pub const NATIVE_PANE_MIN_COLS: u16 = 24;
pub const NATIVE_PANE_MIN_ROWS: u16 = 8;
impl NativeLayout {
    pub fn native_pane_min_extent_for_axis(axis: PaneResizeAxis) -> u16 {
        match axis {
            PaneResizeAxis::Horizontal => NATIVE_PANE_MIN_COLS,
            PaneResizeAxis::Vertical => NATIVE_PANE_MIN_ROWS,
        }
    }

    pub fn native_min_extent_allowed(total_extent: u16, pane_count: usize, min_extent: u16) -> u16 {
        let pane_count = u16::try_from(pane_count).expect("native pane count must fit into u16");
        assert!(pane_count > 0, "native pane count must be non-zero");
        let required = min_extent.saturating_mul(pane_count);
        if total_extent >= required {
            min_extent
        } else {
            (total_extent / pane_count).max(1)
        }
    }
}
