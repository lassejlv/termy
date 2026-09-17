use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativeScaledAxisSpan {
    start: u16,
    end: u16,
}

impl TerminalView {
    pub(in super::super) fn native_pane_lane_count_for_axis(
        panes: &[TerminalPane],
        axis: PaneResizeAxis,
    ) -> usize {
        let mut boundaries = Vec::with_capacity(panes.len().saturating_mul(2));
        for pane in panes {
            let (start, end) = match axis {
                PaneResizeAxis::Horizontal => (pane.left, pane.left.saturating_add(pane.width)),
                PaneResizeAxis::Vertical => (pane.top, pane.top.saturating_add(pane.height)),
            };
            if !boundaries.contains(&start) {
                boundaries.push(start);
            }
            if !boundaries.contains(&end) {
                boundaries.push(end);
            }
        }
        boundaries.sort_unstable();
        boundaries.len().saturating_sub(1)
    }

    pub(in super::super) fn compute_terminal_cols(
        terminal_width: f32,
        cell_width: f32,
        edge_to_edge_grid: bool,
    ) -> u16 {
        let cols = if edge_to_edge_grid {
            (terminal_width / cell_width).ceil()
        } else {
            (terminal_width / cell_width).floor()
        };
        cols.max(2.0) as u16
    }

    pub(in super::super) fn compute_terminal_rows(terminal_height: f32, cell_height: f32) -> u16 {
        (terminal_height / cell_height).floor().max(1.0) as u16
    }

    fn scale_native_pane_edge(edge: u16, old_extent: u16, new_extent: u16) -> u16 {
        if old_extent == 0 || new_extent == 0 {
            return 0;
        }

        let scaled = (u32::from(edge) * u32::from(new_extent)) / u32::from(old_extent);
        scaled.min(u32::from(new_extent)) as u16
    }

    fn scaled_native_pane_axis_span(
        start: u16,
        extent: u16,
        old_extent: u16,
        new_extent: u16,
    ) -> NativeScaledAxisSpan {
        let old_end = start.saturating_add(extent);
        let mut new_start = Self::scale_native_pane_edge(start, old_extent, new_extent)
            .min(new_extent.saturating_sub(1));
        let mut new_end =
            Self::scale_native_pane_edge(old_end, old_extent, new_extent).min(new_extent);
        if new_end <= new_start {
            new_end = (new_start + 1).min(new_extent);
            new_start = new_end.saturating_sub(1);
        }
        NativeScaledAxisSpan {
            start: new_start,
            end: new_end,
        }
    }

    fn rebalance_native_pane_axis_spans(
        spans: &mut [NativeScaledAxisSpan],
        total_extent: u16,
        min_extent: u16,
    ) {
        if spans.is_empty() {
            return;
        }

        let mut boundaries = Vec::with_capacity((spans.len() * 2) + 2);
        boundaries.push(0);
        boundaries.push(total_extent);
        for span in spans.iter() {
            boundaries.push(span.start.min(total_extent));
            boundaries.push(span.end.min(total_extent));
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let pane_boundary_indices = spans
            .iter()
            .map(|span| {
                let start = boundaries
                    .binary_search(&span.start)
                    .expect("native pane rebalance requires span start boundary");
                let end = boundaries
                    .binary_search(&span.end)
                    .expect("native pane rebalance requires span end boundary");
                assert!(
                    start < end,
                    "native pane axis span must have positive extent"
                );
                (start, end)
            })
            .collect::<Vec<_>>();

        // Solve shared boundary positions once per axis so every pane keeps the
        // same boundary graph after scaling while still satisfying the minimum extent.
        let mut min_positions = vec![0u16; boundaries.len()];
        for boundary_index in 1..boundaries.len() {
            let mut required = min_positions[boundary_index - 1];
            for &(start_index, end_index) in &pane_boundary_indices {
                if end_index == boundary_index {
                    required = required.max(min_positions[start_index].saturating_add(min_extent));
                }
            }
            min_positions[boundary_index] = required.min(total_extent);
        }

        let last_index = boundaries.len() - 1;
        let mut max_positions = vec![total_extent; boundaries.len()];
        for boundary_index in (0..last_index).rev() {
            let mut allowed = max_positions[boundary_index + 1];
            for &(start_index, end_index) in &pane_boundary_indices {
                if start_index == boundary_index {
                    allowed = allowed.min(max_positions[end_index].saturating_sub(min_extent));
                }
            }
            max_positions[boundary_index] = allowed;
        }
        debug_assert!(
            min_positions
                .iter()
                .zip(max_positions.iter())
                .all(|(min_position, max_position)| min_position <= max_position)
        );

        let mut adjusted = vec![0u16; boundaries.len()];
        adjusted[0] = 0;
        adjusted[last_index] = total_extent;
        for boundary_index in 1..last_index {
            let lower_bound = min_positions[boundary_index].max(adjusted[boundary_index - 1]);
            let upper_bound = max_positions[boundary_index];
            adjusted[boundary_index] = boundaries[boundary_index].clamp(lower_bound, upper_bound);
        }

        for (span, (start_index, end_index)) in spans.iter_mut().zip(pane_boundary_indices) {
            span.start = adjusted[start_index];
            span.end = adjusted[end_index];
        }
    }

    pub(in super::super) fn sync_native_tab_pane_geometry(
        &mut self,
        tab_id: TabId,
        cols: u16,
        rows: u16,
    ) -> bool {
        if self
            .session
            .native_pane_zoom_snapshots
            .contains_key(&tab_id)
        {
            return false;
        }
        if self.ensure_native_layout_tree_for_tab_id(tab_id)
            && self.apply_native_layout_tree_to_tab(tab_id, cols, rows)
        {
            return true;
        }

        let Some(tab_index) = self.tab_index_by_id(tab_id) else {
            return false;
        };
        let Some(tab) = self.session.tabs.get_mut(tab_index) else {
            return false;
        };
        Self::sync_native_tab_pane_geometry_fallback(tab, cols, rows)
    }

    fn sync_native_tab_pane_geometry_fallback(tab: &mut TerminalTab, cols: u16, rows: u16) -> bool {
        if tab.panes.is_empty() {
            return false;
        }

        let cols = cols.max(1);
        let rows = rows.max(1);

        if tab.panes.len() == 1 {
            if let Some(only) = tab.panes.first_mut() {
                only.left = 0;
                only.top = 0;
                only.width = cols;
                only.height = rows;
            }
            return true;
        }

        let old_cols = tab
            .panes
            .iter()
            .map(|pane| pane.left.saturating_add(pane.width))
            .max()
            .unwrap_or(cols)
            .max(1);
        let old_rows = tab
            .panes
            .iter()
            .map(|pane| pane.top.saturating_add(pane.height))
            .max()
            .unwrap_or(rows)
            .max(1);
        let mut horizontal_spans = tab
            .panes
            .iter()
            .map(|pane| Self::scaled_native_pane_axis_span(pane.left, pane.width, old_cols, cols))
            .collect::<Vec<_>>();
        let mut vertical_spans = tab
            .panes
            .iter()
            .map(|pane| Self::scaled_native_pane_axis_span(pane.top, pane.height, old_rows, rows))
            .collect::<Vec<_>>();
        let min_cols = NativeLayout::native_min_extent_allowed(
            cols,
            tab.panes.len(),
            NativeLayout::native_pane_min_extent_for_axis(PaneResizeAxis::Horizontal),
        );
        let min_rows = NativeLayout::native_min_extent_allowed(
            rows,
            tab.panes.len(),
            NativeLayout::native_pane_min_extent_for_axis(PaneResizeAxis::Vertical),
        );
        Self::rebalance_native_pane_axis_spans(&mut horizontal_spans, cols, min_cols);
        Self::rebalance_native_pane_axis_spans(&mut vertical_spans, rows, min_rows);

        for (pane, (horizontal, vertical)) in tab
            .panes
            .iter_mut()
            .zip(horizontal_spans.into_iter().zip(vertical_spans))
        {
            pane.left = horizontal.start;
            pane.top = vertical.start;
            pane.width = horizontal.end.saturating_sub(horizontal.start).max(1);
            pane.height = vertical.end.saturating_sub(vertical.start).max(1);
        }
        false
    }

    pub(in super::super) fn should_emit_tmux_resize_error_toast(&mut self, now: Instant) -> bool {
        let debounce_window = Duration::from_millis(TMUX_RESIZE_ERROR_TOAST_DEBOUNCE_MS);
        match self.last_tmux_resize_error_at {
            Some(previous) if now.saturating_duration_since(previous) < debounce_window => false,
            _ => {
                self.last_tmux_resize_error_at = Some(now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_terminal_rows_floors_fractional_row_count() {
        assert_eq!(TerminalView::compute_terminal_rows(24.1, 12.0), 2);
        assert_eq!(TerminalView::compute_terminal_rows(23.9, 12.0), 1);
    }

    #[test]
    fn compute_terminal_rows_enforces_minimum_one_row() {
        assert_eq!(TerminalView::compute_terminal_rows(0.5, 12.0), 1);
    }

    #[test]
    fn compute_terminal_cols_preserves_edge_to_edge_ceil_behavior() {
        assert_eq!(TerminalView::compute_terminal_cols(24.1, 12.0, false), 2);
        assert_eq!(TerminalView::compute_terminal_cols(24.1, 12.0, true), 3);
    }

    fn test_terminal() -> Terminal {
        Terminal::new_tmux(TerminalSize::default(), TerminalOptions::default())
    }

    fn test_pane(id: &str, left: u16, top: u16, width: u16, height: u16) -> TerminalPane {
        TerminalPane {
            id: id.to_string(),
            left,
            top,
            width,
            height,
            pane_zoom_steps: 0,
            degraded: false,
            tmux_mouse_mode: None,
            progress_state: ProgressState::default(),
            terminal: test_terminal(),
            render_cache: RefCell::new(TerminalPaneRenderCache::default()),
            last_alternate_screen: Cell::new(false),
            cached_element_ids: PaneCachedElementIds::new(id),
        }
    }

    #[test]
    #[should_panic(expected = "native pane count must be non-zero")]
    fn native_min_extent_allowed_rejects_zero_panes() {
        let _ = NativeLayout::native_min_extent_allowed(10, 0, 2);
    }

    #[test]
    fn native_pane_lane_count_collapses_stacks_on_each_axis() {
        let panes = vec![
            test_pane("%native-1", 0, 0, 60, 20),
            test_pane("%native-2", 0, 20, 60, 20),
            test_pane("%native-3", 60, 0, 60, 40),
        ];

        assert_eq!(
            TerminalView::native_pane_lane_count_for_axis(&panes, PaneResizeAxis::Horizontal),
            2
        );
        assert_eq!(
            TerminalView::native_pane_lane_count_for_axis(&panes, PaneResizeAxis::Vertical),
            2
        );
    }

    #[test]
    fn sync_native_tab_pane_geometry_keeps_existing_active_pane_id() {
        let mut tab = TerminalTab {
            id: 1,
            window_id: "@native-1".to_string(),
            window_index: 0,
            panes: vec![test_pane("%native-1", 0, 0, 40, 20)],
            active_pane_id: "%native-1".to_string(),
            pinned: false,
            manual_title: None,
            explicit_title: None,
            explicit_title_is_prediction: false,
            shell_title: None,
            current_command: None,
            pending_command_title: None,
            pending_command_token: 0,
            last_prompt_cwd: None,
            title: DEFAULT_TAB_TITLE.to_string(),
            title_text_width: 0.0,
            sticky_title_width: 0.0,
            display_width: TAB_MIN_WIDTH,
            running_process: false,
            command_lifecycle: CommandLifecycle::default(),
        };

        TerminalView::sync_native_tab_pane_geometry_fallback(&mut tab, 120, 42);

        assert_eq!(tab.active_pane_id, "%native-1");
        let pane = &tab.panes[0];
        assert_eq!(pane.width, 120);
        assert_eq!(pane.height, 42);
    }

    #[test]
    fn sync_native_tab_pane_geometry_rebalances_widths_to_meet_minimums() {
        let mut tab = TerminalTab {
            id: 1,
            window_id: "@native-1".to_string(),
            window_index: 0,
            panes: vec![
                test_pane("%native-1", 0, 0, 80, 20),
                test_pane("%native-2", 80, 0, 40, 20),
            ],
            active_pane_id: "%native-1".to_string(),
            pinned: false,
            manual_title: None,
            explicit_title: None,
            explicit_title_is_prediction: false,
            shell_title: None,
            current_command: None,
            pending_command_title: None,
            pending_command_token: 0,
            last_prompt_cwd: None,
            title: DEFAULT_TAB_TITLE.to_string(),
            title_text_width: 0.0,
            sticky_title_width: 0.0,
            display_width: TAB_MIN_WIDTH,
            running_process: false,
            command_lifecycle: CommandLifecycle::default(),
        };

        TerminalView::sync_native_tab_pane_geometry_fallback(&mut tab, 45, 20);

        assert_eq!(tab.panes[0].left, 0);
        assert_eq!(tab.panes[1].left, tab.panes[0].width);
        assert_eq!(tab.panes[0].width, 23);
        assert_eq!(tab.panes[1].width, 22);
        assert_eq!(tab.panes.iter().map(|pane| pane.width).sum::<u16>(), 45);
    }

    #[test]
    fn sync_native_tab_pane_geometry_scales_below_default_minimum_when_extent_is_tight() {
        let mut tab = TerminalTab {
            id: 1,
            window_id: "@native-1".to_string(),
            window_index: 0,
            panes: vec![
                test_pane("%native-1", 0, 0, 30, 15),
                test_pane("%native-2", 0, 15, 30, 5),
            ],
            active_pane_id: "%native-1".to_string(),
            pinned: false,
            manual_title: None,
            explicit_title: None,
            explicit_title_is_prediction: false,
            shell_title: None,
            current_command: None,
            pending_command_title: None,
            pending_command_token: 0,
            last_prompt_cwd: None,
            title: DEFAULT_TAB_TITLE.to_string(),
            title_text_width: 0.0,
            sticky_title_width: 0.0,
            display_width: TAB_MIN_WIDTH,
            running_process: false,
            command_lifecycle: CommandLifecycle::default(),
        };

        TerminalView::sync_native_tab_pane_geometry_fallback(&mut tab, 30, 10);

        assert_eq!(tab.panes[0].height, 5);
        assert_eq!(tab.panes[1].top, 5);
        assert_eq!(tab.panes[1].height, 5);
        assert_eq!(tab.panes[0].height + tab.panes[1].height, 10);
    }
}
