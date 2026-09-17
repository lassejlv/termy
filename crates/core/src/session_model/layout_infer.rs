use crate::session_model::layout::*;

pub struct NativePaneInfo {
    pub id: String,
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
}

impl NativeLayout {
    pub fn native_coverage(intervals: &[(u16, u16)], start: u16, end: u16) -> u16 {
        if intervals.is_empty() || start >= end {
            return 0;
        }
        let mut merged = intervals.to_vec();
        merged
            .sort_unstable_by_key(|&(interval_start, interval_end)| (interval_start, interval_end));
        let mut total = 0u16;
        let mut current = merged[0];
        for interval in merged.into_iter().skip(1) {
            if interval.0 <= current.1 {
                current.1 = current.1.max(interval.1);
            } else {
                total = total.saturating_add(current.1.saturating_sub(current.0));
                current = interval;
            }
        }
        total
            .saturating_add(current.1.saturating_sub(current.0))
            .min(end.saturating_sub(start))
    }

    pub fn native_tree_can_split_at_boundary(
        panes: &[&NativePaneInfo],
        rect: NativePaneRect,
        axis: PaneResizeAxis,
        boundary: u16,
    ) -> bool {
        let mut first_count = 0usize;
        let mut second_count = 0usize;
        let mut first_intervals = Vec::new();
        let mut second_intervals = Vec::new();

        for pane in panes {
            let pane_rect = NativePaneRect {
                left: pane.left,
                top: pane.top,
                width: pane.width,
                height: pane.height,
            };
            match axis {
                PaneResizeAxis::Horizontal => {
                    if pane_rect.right() <= boundary {
                        first_count += 1;
                        first_intervals.push((
                            pane_rect.top.max(rect.top),
                            pane_rect.bottom().min(rect.bottom()),
                        ));
                    } else if pane_rect.left >= boundary {
                        second_count += 1;
                        second_intervals.push((
                            pane_rect.top.max(rect.top),
                            pane_rect.bottom().min(rect.bottom()),
                        ));
                    } else {
                        return false;
                    }
                }
                PaneResizeAxis::Vertical => {
                    if pane_rect.bottom() <= boundary {
                        first_count += 1;
                        first_intervals.push((
                            pane_rect.left.max(rect.left),
                            pane_rect.right().min(rect.right()),
                        ));
                    } else if pane_rect.top >= boundary {
                        second_count += 1;
                        second_intervals.push((
                            pane_rect.left.max(rect.left),
                            pane_rect.right().min(rect.right()),
                        ));
                    } else {
                        return false;
                    }
                }
            }
        }

        if first_count == 0 || second_count == 0 {
            return false;
        }

        match axis {
            PaneResizeAxis::Horizontal => {
                Self::native_coverage(&first_intervals, rect.top, rect.bottom()) >= rect.height
                    && Self::native_coverage(&second_intervals, rect.top, rect.bottom())
                        >= rect.height
            }
            PaneResizeAxis::Vertical => {
                Self::native_coverage(&first_intervals, rect.left, rect.right()) >= rect.width
                    && Self::native_coverage(&second_intervals, rect.left, rect.right())
                        >= rect.width
            }
        }
    }

    pub fn native_infer_layout_tree_from_rects(
        panes: &[&NativePaneInfo],
        rect: NativePaneRect,
    ) -> Option<NativePaneLayoutNode> {
        if panes.len() == 1 {
            return Some(NativePaneLayoutNode::Leaf {
                pane_id: panes[0].id.clone(),
            });
        }

        let right_boundaries = panes
            .iter()
            .map(|pane| pane.left.saturating_add(pane.width))
            .filter(|boundary| *boundary > rect.left && *boundary < rect.right())
            .collect::<Vec<_>>();
        for boundary in right_boundaries {
            if !Self::native_tree_can_split_at_boundary(
                panes,
                rect,
                PaneResizeAxis::Horizontal,
                boundary,
            ) {
                continue;
            }
            let (first_panes, second_panes): (Vec<_>, Vec<_>) = panes
                .iter()
                .copied()
                .partition(|pane| pane.left.saturating_add(pane.width) <= boundary);
            let first_rect = NativePaneRect {
                width: boundary.saturating_sub(rect.left),
                ..rect
            };
            let second_rect = NativePaneRect {
                left: boundary,
                width: rect.right().saturating_sub(boundary),
                ..rect
            };
            let first = Self::native_infer_layout_tree_from_rects(&first_panes, first_rect)?;
            let second = Self::native_infer_layout_tree_from_rects(&second_panes, second_rect)?;
            return Some(NativePaneLayoutNode::Split {
                axis: PaneResizeAxis::Horizontal,
                ratio: f32::from(first_rect.width) / f32::from(rect.width.max(1)),
                first: Box::new(first),
                second: Box::new(second),
            });
        }

        let bottom_boundaries = panes
            .iter()
            .map(|pane| pane.top.saturating_add(pane.height))
            .filter(|boundary| *boundary > rect.top && *boundary < rect.bottom())
            .collect::<Vec<_>>();
        for boundary in bottom_boundaries {
            if !Self::native_tree_can_split_at_boundary(
                panes,
                rect,
                PaneResizeAxis::Vertical,
                boundary,
            ) {
                continue;
            }
            let (first_panes, second_panes): (Vec<_>, Vec<_>) = panes
                .iter()
                .copied()
                .partition(|pane| pane.top.saturating_add(pane.height) <= boundary);
            let first_rect = NativePaneRect {
                height: boundary.saturating_sub(rect.top),
                ..rect
            };
            let second_rect = NativePaneRect {
                top: boundary,
                height: rect.bottom().saturating_sub(boundary),
                ..rect
            };
            let first = Self::native_infer_layout_tree_from_rects(&first_panes, first_rect)?;
            let second = Self::native_infer_layout_tree_from_rects(&second_panes, second_rect)?;
            return Some(NativePaneLayoutNode::Split {
                axis: PaneResizeAxis::Vertical,
                ratio: f32::from(first_rect.height) / f32::from(rect.height.max(1)),
                first: Box::new(first),
                second: Box::new(second),
            });
        }

        None
    }
}
