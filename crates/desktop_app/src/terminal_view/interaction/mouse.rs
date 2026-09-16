use super::*;

const SELECTION_DRAG_AUTOSCROLL_MAX_LINES: i32 = 3;
const CURSOR_MOVE_PREVIEW_MS: u64 = 75;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal_view) struct PendingCursorMoveClick {
    pub(in crate::terminal_view) pane_id: String,
    pub(in crate::terminal_view) selection_start: SelectionPos,
    pub(in crate::terminal_view) start_cell: CellPos,
    pub(in crate::terminal_view) target: CellPos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal_view) struct PendingCursorMovePreview {
    pub(in crate::terminal_view) pane_id: String,
    pub(in crate::terminal_view) target: CellPos,
    pub(in crate::terminal_view) style: TerminalCursorStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GlobalTabDragPointerAction {
    None,
    UpdatePreview,
    Commit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectionReleaseAction {
    KeepImage,
    MoveCursor,
    FinishTextSelection,
}

impl TerminalView {
    fn selection_release_action(
        selection_moved: bool,
        image_selected: bool,
    ) -> SelectionReleaseAction {
        if selection_moved {
            SelectionReleaseAction::FinishTextSelection
        } else if image_selected {
            SelectionReleaseAction::KeepImage
        } else {
            SelectionReleaseAction::MoveCursor
        }
    }

    fn global_tab_drag_pointer_action(
        tab_drag_active: bool,
        pointer_dragging: bool,
    ) -> GlobalTabDragPointerAction {
        if !tab_drag_active {
            return GlobalTabDragPointerAction::None;
        }
        if pointer_dragging {
            GlobalTabDragPointerAction::UpdatePreview
        } else {
            GlobalTabDragPointerAction::Commit
        }
    }

    fn is_plain_click_cursor_move_gesture(modifiers: gpui::Modifiers, click_count: usize) -> bool {
        click_count == 1
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.shift
            && !modifiers.platform
            && !modifiers.function
    }

    fn click_cursor_move_allowed_for_state(
        runtime_kind: RuntimeKind,
        running_process: bool,
        has_current_command: bool,
        alternate_screen_mode: bool,
    ) -> bool {
        if running_process || alternate_screen_mode {
            return false;
        }

        match runtime_kind {
            RuntimeKind::Tmux => true,
            RuntimeKind::Native => !has_current_command,
        }
    }

    fn cursor_move_input_for_click_target(cursor: CellPos, target: CellPos) -> Option<Vec<u8>> {
        if cursor.row != target.row || cursor.col == target.col {
            return None;
        }

        let (sequence, repeats) = if target.col > cursor.col {
            (b"\x1b[C".as_slice(), target.col - cursor.col)
        } else {
            (b"\x1b[D".as_slice(), cursor.col - target.col)
        };

        let mut input = Vec::with_capacity(sequence.len() * repeats);
        for _ in 0..repeats {
            input.extend_from_slice(sequence);
        }
        Some(input)
    }

    fn pending_cursor_move_click_for_mouse_down(
        &self,
        event: &MouseDownEvent,
    ) -> Option<PendingCursorMoveClick> {
        if event.button != MouseButton::Left
            || !Self::is_plain_click_cursor_move_gesture(event.modifiers, event.click_count)
        {
            return None;
        }

        let (pane_id, target) = self.position_to_pane_cell(event.position, false)?;
        if !self.pane_cell_has_clickable_text(pane_id.as_str(), target) {
            return None;
        }
        let selection_start = self.selection_pos_for_pane_cell(pane_id.as_str(), target)?;
        let terminal = self.pane_terminal_by_id(pane_id.as_str())?;
        let tab = self.session.tabs.get(self.session.active_tab)?;
        let runtime_kind = self.runtime_kind();
        if !Self::click_cursor_move_allowed_for_state(
            runtime_kind,
            tab.running_process,
            tab.current_command.is_some(),
            terminal.alternate_screen_mode(),
        ) {
            return None;
        }

        Some(PendingCursorMoveClick {
            pane_id,
            selection_start,
            start_cell: target,
            target,
        })
    }

    fn begin_selection_drag_from_pending_cursor_move(
        &mut self,
        position: gpui::Point<Pixels>,
    ) -> bool {
        let Some(pending) = self.pending_cursor_move_click.as_ref() else {
            return false;
        };
        let Some((pane_id, cell)) = self.position_to_pane_cell(position, true) else {
            return false;
        };
        if !Self::pending_cursor_move_starts_selection(pending, pane_id.as_str(), cell) {
            return false;
        }

        let Some(selection_head) = self.selection_pos_for_pane_cell(pane_id.as_str(), cell) else {
            return false;
        };
        let selection_start = pending.selection_start;
        self.pending_cursor_move_click = None;
        self.selection_anchor = Some(selection_start);
        self.selection_head = Some(selection_head);
        self.selection_dragging = true;
        self.selection_moved = selection_start != selection_head;
        true
    }

    fn pending_cursor_move_starts_selection(
        pending: &PendingCursorMoveClick,
        pane_id: &str,
        cell: CellPos,
    ) -> bool {
        pending.pane_id == pane_id && cell != pending.start_cell
    }

    fn maybe_move_cursor_to_click_target(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_cursor_move_click.take() else {
            return;
        };
        if !self.is_active_pane_id(pending.pane_id.as_str()) {
            return;
        }

        let Some(terminal) = self.pane_terminal_by_id(pending.pane_id.as_str()) else {
            return;
        };
        let (cursor_col, cursor_row) = terminal.cursor_position();
        let Some(input) = Self::cursor_move_input_for_click_target(
            CellPos {
                col: cursor_col,
                row: cursor_row,
            },
            pending.target,
        ) else {
            return;
        };

        let cursor_style = terminal
            .cursor_state()
            .map_or(TerminalCursorStyle::Block, |cursor_state| {
                cursor_state.style
            });
        self.start_cursor_move_preview(
            PendingCursorMovePreview {
                pane_id: pending.pane_id,
                target: pending.target,
                style: cursor_style,
            },
            cx,
        );
        self.write_terminal_input(&input, cx);
    }

    fn start_cursor_move_preview(
        &mut self,
        preview: PendingCursorMovePreview,
        cx: &mut Context<Self>,
    ) {
        self.pending_cursor_move_preview = Some(preview.clone());
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(CURSOR_MOVE_PREVIEW_MS)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    if view.pending_cursor_move_preview.as_ref() == Some(&preview) {
                        view.pending_cursor_move_preview = None;
                        cx.notify();
                    }
                })
            });
        })
        .detach();
    }

    fn finish_pending_cursor_move_click(
        &mut self,
        button: MouseButton,
        cx: &mut Context<Self>,
    ) -> bool {
        if button != MouseButton::Left || self.pending_cursor_move_click.is_none() {
            return false;
        }

        let selection_changed = self.clear_selection();
        let hovered_link_changed = self.clear_hovered_link();
        self.maybe_move_cursor_to_click_target(cx);
        if selection_changed || hovered_link_changed {
            cx.notify();
        }
        true
    }

    fn native_resize_overlap_cells(a_start: u16, a_end: u16, b_start: u16, b_end: u16) -> u16 {
        let start = a_start.max(b_start);
        let end = a_end.min(b_end);
        end.saturating_sub(start)
    }

    pub(in super::super) fn begin_pane_resize_drag(
        &mut self,
        pane_id: &str,
        axis: PaneResizeAxis,
        edge: PaneResizeEdge,
        position: gpui::Point<Pixels>,
    ) -> bool {
        let (x, y) = self.terminal_content_position(position);
        self.pane_resize_drag = Some(PaneResizeDragState {
            pane_id: pane_id.to_string(),
            axis,
            edge,
            start_x: x,
            start_y: y,
            applied_steps: 0,
        });
        true
    }

    fn finish_pane_resize_drag(&mut self, cx: &mut Context<Self>) -> bool {
        if self.pane_resize_drag.take().is_none() {
            return false;
        }
        self.pane_resize_blocked = false;
        self.last_terminal_resize_signature = None;
        self.last_resize_applied_at = None;
        if self.runtime_kind() == RuntimeKind::Native {
            self.schedule_persist_native_workspace(cx);
        }
        true
    }

    fn pane_resize_hit_test(
        &self,
        position: gpui::Point<Pixels>,
        window: &Window,
    ) -> Option<PaneResizeDragState> {
        let tab = self.session.tabs.get(self.session.active_tab)?;
        let content_bounds = self.terminal_content_bounds(window)?;
        let (x, y) = self.terminal_content_position(position);
        self.native_pane_dividers(tab, content_bounds)
            .into_iter()
            .filter_map(|divider| {
                divider
                    .hit_distance(x, y)
                    .map(|distance| (distance, divider))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, divider)| PaneResizeDragState {
                pane_id: divider.pane_id,
                axis: divider.axis,
                edge: divider.edge,
                start_x: x,
                start_y: y,
                applied_steps: 0,
            })
    }

    pub(in crate::terminal_view) fn native_resize_pane_step(
        &mut self,
        pane_id: &str,
        axis: PaneResizeAxis,
        edge: PaneResizeEdge,
        divider_delta: i16,
    ) -> PaneResizeResult {
        fn overlaps_any(span_start: u16, span_end: u16, spans: &[(u16, u16)]) -> bool {
            spans.iter().any(|(start, end)| {
                TerminalView::native_resize_overlap_cells(span_start, span_end, *start, *end) > 0
            })
        }

        if divider_delta == 0 {
            return PaneResizeResult::NoChange;
        }

        let (
            tab_id,
            total_cols,
            total_rows,
            target_id,
            target_left,
            target_top,
            target_width,
            target_height,
        ) = {
            let Some(tab) = self.session.tabs.get(self.session.active_tab) else {
                return PaneResizeResult::NoChange;
            };
            let Some(target_index) = tab.panes.iter().position(|pane| pane.id == pane_id) else {
                return PaneResizeResult::NoChange;
            };
            let Some(target) = tab.panes.get(target_index) else {
                return PaneResizeResult::NoChange;
            };
            (
                tab.id,
                tab.panes
                    .iter()
                    .map(|pane| pane.left.saturating_add(pane.width))
                    .max()
                    .unwrap_or(target.width)
                    .max(1),
                tab.panes
                    .iter()
                    .map(|pane| pane.top.saturating_add(pane.height))
                    .max()
                    .unwrap_or(target.height)
                    .max(1),
                target.id.clone(),
                target.left,
                target.top,
                target.width,
                target.height,
            )
        };

        if self.ensure_native_layout_tree_for_tab_id(tab_id)
            && let Some(tree) = self.session.native_pane_layout_trees.get_mut(&tab_id)
        {
            let min_extent = match axis {
                PaneResizeAxis::Horizontal => Self::native_min_extent_allowed(
                    total_cols,
                    Self::native_tree_leaf_count(&tree.root),
                    Self::native_pane_min_extent_for_axis(PaneResizeAxis::Horizontal),
                ),
                PaneResizeAxis::Vertical => Self::native_min_extent_allowed(
                    total_rows,
                    Self::native_tree_leaf_count(&tree.root),
                    Self::native_pane_min_extent_for_axis(PaneResizeAxis::Vertical),
                ),
            };
            let result = Self::native_adjust_tree_split(
                &mut tree.root,
                target_id.as_str(),
                axis,
                edge,
                divider_delta,
                NativePaneRect {
                    left: 0,
                    top: 0,
                    width: total_cols,
                    height: total_rows,
                },
                min_extent,
            );
            if result != PaneResizeResult::NoChange {
                self.apply_native_layout_tree_to_tab(tab_id, total_cols, total_rows);
                return result;
            }
        }

        let Some(tab) = self.session.tabs.get_mut(self.session.active_tab) else {
            return PaneResizeResult::NoChange;
        };
        debug_assert_eq!(tab.id, tab_id);

        match axis {
            PaneResizeAxis::Horizontal => {
                let boundary = match edge {
                    PaneResizeEdge::Left => target_left,
                    PaneResizeEdge::Right => target_left.saturating_add(target_width),
                    PaneResizeEdge::Top | PaneResizeEdge::Bottom => {
                        return PaneResizeResult::NoChange;
                    }
                };
                let mut spans = vec![(target_top, target_top.saturating_add(target_height))];
                let mut left_indices = Vec::<usize>::new();
                let mut right_indices = Vec::<usize>::new();

                loop {
                    let mut changed = false;
                    for (index, pane) in tab.panes.iter().enumerate() {
                        let pane_top = pane.top;
                        let pane_bottom = pane.top.saturating_add(pane.height);
                        if !overlaps_any(pane_top, pane_bottom, &spans) {
                            continue;
                        }

                        let pane_left = pane.left;
                        let pane_right = pane.left.saturating_add(pane.width);
                        if pane_right == boundary && !left_indices.contains(&index) {
                            left_indices.push(index);
                            spans.push((pane_top, pane_bottom));
                            changed = true;
                        }
                        if pane_left == boundary && !right_indices.contains(&index) {
                            right_indices.push(index);
                            spans.push((pane_top, pane_bottom));
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }

                if left_indices.is_empty() || right_indices.is_empty() {
                    return PaneResizeResult::NoChange;
                }
                let horizontal_lane_count =
                    Self::native_pane_lane_count_for_axis(&tab.panes, PaneResizeAxis::Horizontal);
                let min_width = Self::native_min_extent_allowed(
                    tab.panes
                        .iter()
                        .map(|pane| pane.left.saturating_add(pane.width))
                        .max()
                        .unwrap_or(0),
                    horizontal_lane_count,
                    Self::native_pane_min_extent_for_axis(PaneResizeAxis::Horizontal),
                );

                if divider_delta > 0 {
                    if right_indices.iter().any(|index| {
                        tab.panes[*index].width
                            < min_width.saturating_add(divider_delta.unsigned_abs())
                    }) {
                        return PaneResizeResult::BlockedByMinimum;
                    }
                    for index in left_indices {
                        tab.panes[index].width =
                            tab.panes[index].width.saturating_add(divider_delta as u16);
                    }
                    for index in right_indices {
                        tab.panes[index].left =
                            tab.panes[index].left.saturating_add(divider_delta as u16);
                        tab.panes[index].width =
                            tab.panes[index].width.saturating_sub(divider_delta as u16);
                    }
                } else {
                    let shrink = divider_delta.unsigned_abs();
                    if left_indices
                        .iter()
                        .any(|index| tab.panes[*index].width < min_width.saturating_add(shrink))
                    {
                        return PaneResizeResult::BlockedByMinimum;
                    }
                    for index in left_indices {
                        tab.panes[index].width = tab.panes[index].width.saturating_sub(shrink);
                    }
                    for index in right_indices {
                        tab.panes[index].left = tab.panes[index].left.saturating_sub(shrink);
                        tab.panes[index].width = tab.panes[index].width.saturating_add(shrink);
                    }
                }
            }
            PaneResizeAxis::Vertical => {
                let boundary = match edge {
                    PaneResizeEdge::Top => target_top,
                    PaneResizeEdge::Bottom => target_top.saturating_add(target_height),
                    PaneResizeEdge::Left | PaneResizeEdge::Right => {
                        return PaneResizeResult::NoChange;
                    }
                };
                let mut spans = vec![(target_left, target_left.saturating_add(target_width))];
                let mut top_indices = Vec::<usize>::new();
                let mut bottom_indices = Vec::<usize>::new();

                loop {
                    let mut changed = false;
                    for (index, pane) in tab.panes.iter().enumerate() {
                        let pane_left = pane.left;
                        let pane_right = pane.left.saturating_add(pane.width);
                        if !overlaps_any(pane_left, pane_right, &spans) {
                            continue;
                        }

                        let pane_top = pane.top;
                        let pane_bottom = pane.top.saturating_add(pane.height);
                        if pane_bottom == boundary && !top_indices.contains(&index) {
                            top_indices.push(index);
                            spans.push((pane_left, pane_right));
                            changed = true;
                        }
                        if pane_top == boundary && !bottom_indices.contains(&index) {
                            bottom_indices.push(index);
                            spans.push((pane_left, pane_right));
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }

                if top_indices.is_empty() || bottom_indices.is_empty() {
                    return PaneResizeResult::NoChange;
                }
                let vertical_lane_count =
                    Self::native_pane_lane_count_for_axis(&tab.panes, PaneResizeAxis::Vertical);
                let min_height = Self::native_min_extent_allowed(
                    tab.panes
                        .iter()
                        .map(|pane| pane.top.saturating_add(pane.height))
                        .max()
                        .unwrap_or(0),
                    vertical_lane_count,
                    Self::native_pane_min_extent_for_axis(PaneResizeAxis::Vertical),
                );

                if divider_delta > 0 {
                    if bottom_indices.iter().any(|index| {
                        tab.panes[*index].height
                            < min_height.saturating_add(divider_delta.unsigned_abs())
                    }) {
                        return PaneResizeResult::BlockedByMinimum;
                    }
                    for index in top_indices {
                        tab.panes[index].height =
                            tab.panes[index].height.saturating_add(divider_delta as u16);
                    }
                    for index in bottom_indices {
                        tab.panes[index].top =
                            tab.panes[index].top.saturating_add(divider_delta as u16);
                        tab.panes[index].height =
                            tab.panes[index].height.saturating_sub(divider_delta as u16);
                    }
                } else {
                    let shrink = divider_delta.unsigned_abs();
                    if top_indices
                        .iter()
                        .any(|index| tab.panes[*index].height < min_height.saturating_add(shrink))
                    {
                        return PaneResizeResult::BlockedByMinimum;
                    }
                    for index in top_indices {
                        tab.panes[index].height = tab.panes[index].height.saturating_sub(shrink);
                    }
                    for index in bottom_indices {
                        tab.panes[index].top = tab.panes[index].top.saturating_sub(shrink);
                        tab.panes[index].height = tab.panes[index].height.saturating_add(shrink);
                    }
                }
            }
        }

        PaneResizeResult::Applied
    }

    fn apply_pane_resize_drag(&mut self, position: gpui::Point<Pixels>) -> bool {
        let Some(drag_state) = self.pane_resize_drag.as_ref() else {
            return false;
        };
        let pane_id = drag_state.pane_id.clone();
        let axis = drag_state.axis;
        let edge = drag_state.edge;
        let start_x = drag_state.start_x;
        let start_y = drag_state.start_y;
        let already_applied_steps = drag_state.applied_steps;

        let layout_cell_size = self.layout_cell_size();
        let axis_cell_pixels = match axis {
            PaneResizeAxis::Horizontal => {
                let width: f32 = layout_cell_size.width.into();
                width
            }
            PaneResizeAxis::Vertical => {
                let height: f32 = layout_cell_size.height.into();
                height
            }
        };
        if axis_cell_pixels <= f32::EPSILON {
            return false;
        }

        let (current_x, current_y) = self.terminal_content_position(position);
        let delta_pixels = match axis {
            PaneResizeAxis::Horizontal => current_x - start_x,
            PaneResizeAxis::Vertical => current_y - start_y,
        };
        let desired_steps = (delta_pixels / axis_cell_pixels).trunc() as i32;
        let step_delta = desired_steps - already_applied_steps;
        if step_delta == 0 {
            return false;
        }
        let (completed_steps, was_blocked) = match self.runtime_kind() {
            RuntimeKind::Tmux => {
                // Left/top drags invert the tmux resize direction relative to cursor delta,
                // because dragging toward the pane interior shrinks that edge.
                let positive_direction = match edge {
                    PaneResizeEdge::Left | PaneResizeEdge::Top => step_delta.is_negative(),
                    PaneResizeEdge::Right | PaneResizeEdge::Bottom => step_delta.is_positive(),
                };
                let cells = step_delta.unsigned_abs().min(u32::from(u16::MAX)) as u16;
                if self.tmux_resize_pane_by(pane_id.as_str(), axis, positive_direction, cells) {
                    (i32::from(cells), false)
                } else {
                    (0, false)
                }
            }
            RuntimeKind::Native => {
                let batched_delta = step_delta.clamp(i32::from(i16::MIN), i32::from(i16::MAX));
                match self.native_resize_pane_step(
                    pane_id.as_str(),
                    axis,
                    edge,
                    batched_delta as i16,
                ) {
                    PaneResizeResult::Applied => (batched_delta.unsigned_abs() as i32, false),
                    PaneResizeResult::NoChange => (0, false),
                    PaneResizeResult::BlockedByMinimum => {
                        let mut completed_steps = 0i32;
                        for _ in 0..batched_delta.unsigned_abs() {
                            match self.native_resize_pane_step(
                                pane_id.as_str(),
                                axis,
                                edge,
                                if batched_delta.is_positive() { 1 } else { -1 },
                            ) {
                                PaneResizeResult::Applied => completed_steps += 1,
                                PaneResizeResult::BlockedByMinimum | PaneResizeResult::NoChange => {
                                    break;
                                }
                            }
                        }
                        (completed_steps, true)
                    }
                }
            }
        };

        self.pane_resize_blocked = was_blocked;

        if completed_steps == 0 {
            return false;
        }

        let applied_delta = if step_delta.is_positive() {
            completed_steps
        } else {
            -completed_steps
        };
        if let Some(drag) = self.pane_resize_drag.as_mut() {
            drag.applied_steps += applied_delta;
        }
        true
    }

    pub(in super::super) fn selection_drag_autoscroll_lines_from_bounds(
        pointer_y: f32,
        top: f32,
        bottom: f32,
        line_height: f32,
    ) -> i32 {
        if line_height <= f32::EPSILON || top >= bottom {
            return 0;
        }

        let lines = if pointer_y < top {
            // Positive deltas move up into history.
            ((top - pointer_y).powf(1.1) / line_height).ceil() as i32
        } else if pointer_y > bottom {
            // Negative deltas move down toward live output.
            -((pointer_y - bottom).powf(1.1) / line_height).ceil() as i32
        } else {
            0
        };

        lines.clamp(
            -SELECTION_DRAG_AUTOSCROLL_MAX_LINES,
            SELECTION_DRAG_AUTOSCROLL_MAX_LINES,
        )
    }

    fn selection_drag_autoscroll_lines(&self, position: gpui::Point<Pixels>) -> i32 {
        let Some(geometry) = self.terminal_viewport_geometry() else {
            return 0;
        };
        let Some(terminal) = self.active_terminal() else {
            return 0;
        };
        let line_height: f32 = terminal.size().cell_height;
        let (_, pointer_y) = self.terminal_content_position(position);
        let top = geometry.origin_y;
        let bottom = geometry.origin_y + geometry.height.max(0.0);
        Self::selection_drag_autoscroll_lines_from_bounds(pointer_y, top, bottom, line_height)
    }

    pub(in super::super) fn update_selection_head_from_position(
        &mut self,
        position: gpui::Point<Pixels>,
        clamp: bool,
    ) -> bool {
        let Some(next) = self.position_to_selection_pos(position, clamp) else {
            return false;
        };

        if self.selection_head != Some(next) {
            self.selection_head = Some(next);
            if self.selection_anchor != self.selection_head {
                self.selection_moved = true;
                self.kitty_image_selection = None;
            }
            true
        } else {
            false
        }
    }

    fn handle_selection_drag_motion(
        &mut self,
        position: gpui::Point<Pixels>,
        allow_autoscroll: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.selection_dragging {
            return false;
        }

        let mut changed = false;
        if allow_autoscroll {
            let delta_lines = self.selection_drag_autoscroll_lines(position);
            if delta_lines != 0
                && self
                    .active_terminal()
                    .is_some_and(|terminal| terminal.scroll_display(delta_lines))
            {
                self.sync_content_scroll_baseline();
                self.mark_terminal_scrollbar_activity(cx);
                changed = true;
            }
        }

        if self.update_selection_head_from_position(position, true) {
            changed = true;
        }

        if changed {
            self.clear_hovered_link();
            cx.notify();
        }
        changed
    }

    fn finish_selection_drag_at_position(
        &mut self,
        position: Option<gpui::Point<Pixels>>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.selection_dragging {
            return false;
        }

        if let Some(position) = position {
            self.update_selection_head_from_position(position, true);
        }

        self.selection_dragging = false;
        match Self::selection_release_action(
            self.selection_moved,
            self.kitty_image_selection.is_some(),
        ) {
            SelectionReleaseAction::KeepImage => {
                self.clear_text_selection();
                self.pending_cursor_move_click = None;
            }
            SelectionReleaseAction::MoveCursor => {
                self.clear_text_selection();
                self.maybe_move_cursor_to_click_target(cx);
            }
            SelectionReleaseAction::FinishTextSelection => {
                self.pending_cursor_move_click = None;
                self.copy_selection_to_clipboard_if_enabled(cx);
            }
        }
        self.clear_hovered_link();
        cx.notify();
        true
    }

    fn copy_selection_to_clipboard_if_enabled(&mut self, cx: &mut Context<Self>) {
        if self.copy_on_select
            && let Some(text) = self.selected_text()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            if self.copy_on_select_toast {
                crate::ui::toast::enqueue_toast(
                    crate::ui::toast::ToastKind::Success,
                    "Copied",
                    Some(std::time::Duration::from_millis(1500)),
                );
            }
        }
    }

    fn should_finish_selection_drag(button: MouseButton, selection_dragging: bool) -> bool {
        button == MouseButton::Left && selection_dragging
    }

    pub(in super::super) fn handle_global_mouse_move_event(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_sidebar_resize_drag_active() {
            if event.dragging() {
                if self.update_workspace_sidebar_resize_drag(event.position.x.into()) {
                    cx.notify();
                }
            } else if self.finish_workspace_sidebar_resize_drag() {
                cx.notify();
            }
            return;
        }

        if self.inspector_resize_drag_active() {
            if !event.dragging() {
                let _ = self.finish_inspector_resize_drag();
                return;
            }
            if self.update_inspector_resize_drag(event.position.y.into()) {
                cx.notify();
            }
            return;
        }

        if self.workspace_drag.is_some() {
            if !event.dragging() {
                self.commit_workspace_drag(cx);
            }
            return;
        }

        if self.pane_move_drag.is_some() {
            if event.dragging() {
                self.update_pane_move_drag(event.position, window, cx);
            } else {
                let _ = self.finish_pane_move_drag(cx);
            }
            return;
        }

        match Self::global_tab_drag_pointer_action(self.tab_strip.drag.is_some(), event.dragging())
        {
            GlobalTabDragPointerAction::None => {}
            GlobalTabDragPointerAction::UpdatePreview => {
                let Some(orientation) = self.tab_strip.drag.map(|drag| drag.orientation) else {
                    return;
                };
                let preview = self.tab_strip_drag_preview(orientation, window, event.position);
                self.update_tab_drag_preview(
                    orientation,
                    preview.pointer_primary_axis,
                    preview.viewport_extent,
                    cx,
                );
                return;
            }
            GlobalTabDragPointerAction::Commit => {
                self.commit_tab_drag(cx);
                return;
            }
        }

        if self.try_forward_mouse_move(event, cx) {
            return;
        }

        if event.pressed_button == Some(MouseButton::Left)
            && self.begin_selection_drag_from_pending_cursor_move(event.position)
        {
            self.clear_hovered_link();
            cx.notify();
            return;
        }

        if event.pressed_button != Some(MouseButton::Left) || !self.selection_dragging {
            return;
        }

        self.handle_selection_drag_motion(event.position, true, cx);
    }

    pub(in super::super) fn handle_global_mouse_up_event(
        &mut self,
        event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button == MouseButton::Left && self.finish_workspace_sidebar_resize_drag() {
            cx.notify();
            return true;
        }

        if event.button == MouseButton::Left && self.finish_inspector_resize_drag() {
            cx.notify();
            return true;
        }

        if event.button == MouseButton::Left && self.workspace_drag.is_some() {
            self.commit_workspace_drag(cx);
            return true;
        }

        if event.button == MouseButton::Left
            && self.pane_move_drag.is_some()
            && self.finish_pane_move_drag(cx)
        {
            return true;
        }

        if event.button == MouseButton::Left && self.finish_pane_resize_drag(cx) {
            cx.notify();
            return true;
        }

        if event.button == MouseButton::Left && self.tab_strip.drag.is_some() {
            self.commit_tab_drag(cx);
            return true;
        }

        if self.try_forward_mouse_up(event, cx) {
            return true;
        }

        if self.finish_pending_cursor_move_click(event.button, cx) {
            return true;
        }

        if !Self::should_finish_selection_drag(event.button, self.selection_dragging) {
            return false;
        }

        self.finish_selection_drag_at_position(Some(event.position), cx)
    }

    pub(in super::super) fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_cursor_move_click = None;
        self.pending_cursor_move_preview = None;

        // Focus the terminal on click
        self.focus_handle.focus(window);
        self.reset_cursor_blink_phase();
        if event.button != MouseButton::Right {
            let _ = self.close_terminal_context_menu(cx);
        }
        let mut changed = false;
        if event.button == MouseButton::Left && self.tab_strip.drag.is_some() {
            self.commit_tab_drag(cx);
        } else if self.reset_tab_drag_state() {
            changed = true;
        }
        if self.clear_tab_hover_state() {
            changed = true;
        }
        if changed {
            cx.notify();
        }

        if event.button == MouseButton::Right {
            if self.try_forward_mouse_down(event, cx) {
                let _ = self.close_terminal_context_menu(cx);
                return;
            }

            if let Some((pane_id, _)) = self.position_to_pane_cell(event.position, false)
                && !self.is_active_pane_id(pane_id.as_str())
            {
                let _ = self.focus_pane_target(pane_id.as_str(), cx);
            }
            let image_selection = self.kitty_image_at_position(event.position);
            if self.kitty_image_selection != image_selection {
                self.kitty_image_selection = image_selection;
                cx.notify();
            }
            self.open_terminal_context_menu_for_window(event.position, window, cx);
            cx.stop_propagation();
            return;
        }

        // Pane-move drags take priority over app mouse reporting so the
        // modifier combo always works, even inside fullscreen TUI apps.
        if event.button == MouseButton::Left
            && Self::is_pane_move_modifier(event.modifiers)
            && self.try_begin_pane_move_drag(event.position, window, cx)
        {
            cx.stop_propagation();
            return;
        }

        if self.try_forward_mouse_down(event, cx) {
            return;
        }

        if event.button != MouseButton::Left {
            return;
        }

        if let Some(drag) = self.pane_resize_hit_test(event.position, window) {
            self.pane_resize_drag = Some(drag);
            cx.stop_propagation();
            return;
        }

        if let Some(hit) = self.terminal_scrollbar_hit_test(event.position, window) {
            self.handle_terminal_scrollbar_mouse_down(hit, window, cx);
            cx.stop_propagation();
            return;
        }

        if let Some((pane_id, _)) = self.position_to_pane_cell(event.position, false)
            && !self.is_active_pane_id(pane_id.as_str())
        {
            let focused = self.focus_pane_target(pane_id.as_str(), cx);
            if focused && self.clear_hovered_link() {
                cx.notify();
            }
        }

        let image_selection = self.kitty_image_at_position_for_left_click(event.position);
        let has_image_selection = image_selection.is_some();
        let image_selection_changed = self.kitty_image_selection != image_selection;
        self.kitty_image_selection = image_selection;

        if !has_image_selection
            && Self::is_link_modifier(event.modifiers)
            && let Some(cell) = self.position_to_cell(event.position, false)
            && let Some(link) = self.link_at_cell(cell)
        {
            if !Self::open_link(&link.target) {
                crate::ui::toast::error("Failed to open link");
            }
            if self.clear_hovered_link() || image_selection_changed {
                cx.notify();
            }
            return;
        }

        let Some(cell) = self.position_to_cell(event.position, false) else {
            self.clear_selection();
            self.clear_hovered_link();
            cx.notify();
            return;
        };

        if !has_image_selection && event.click_count >= 3 && self.select_line_at_row(cell.row) {
            self.copy_selection_to_clipboard_if_enabled(cx);
            self.clear_hovered_link();
            cx.notify();
            return;
        }

        if !has_image_selection && event.click_count == 2 && self.select_token_at_cell(cell) {
            self.copy_selection_to_clipboard_if_enabled(cx);
            self.clear_hovered_link();
            cx.notify();
            return;
        }

        let Some(selection_pos) = self.selection_pos_for_cell(cell) else {
            self.clear_selection();
            self.clear_hovered_link();
            cx.notify();
            return;
        };

        self.pending_cursor_move_click = if has_image_selection {
            None
        } else {
            self.pending_cursor_move_click_for_mouse_down(event)
        };
        if self.pending_cursor_move_click.is_some() {
            if self.clear_hovered_link() {
                cx.notify();
            }
            return;
        }

        self.selection_anchor = Some(selection_pos);
        self.selection_head = Some(selection_pos);
        self.selection_dragging = true;
        self.selection_moved = false;
        self.clear_hovered_link();
        cx.notify();
    }

    pub(in super::super) fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_sidebar_resize_drag_active() {
            if event.dragging() {
                if self.update_workspace_sidebar_resize_drag(event.position.x.into()) {
                    cx.notify();
                }
            } else if self.finish_workspace_sidebar_resize_drag() {
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }

        if self.tab_strip.drag.is_some() && !event.dragging() {
            self.commit_tab_drag(cx);
        }
        if self.workspace_drag.is_some() && !event.dragging() {
            self.commit_workspace_drag(cx);
        }

        if self.clear_tab_hover_state() {
            cx.notify();
        }

        if self.terminal_scrollbar_drag.is_some() {
            if event.dragging() {
                self.handle_terminal_scrollbar_drag(event.position, window, cx);
            } else if self.finish_terminal_scrollbar_drag(cx) {
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if event.dragging() && self.terminal_scrollbar_track_hold.is_some() {
            if let Some(hit) = self.terminal_scrollbar_hit_test(event.position, window) {
                self.update_terminal_scrollbar_track_hold(hit.local_y);
                cx.stop_propagation();
                return;
            }
            if self.stop_terminal_scrollbar_track_hold() {
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        if self.pane_move_drag.is_some() {
            if event.dragging() {
                self.update_pane_move_drag(event.position, window, cx);
            } else {
                // The matching mouse-up was missed (e.g. released outside the
                // window); resolve the drag now.
                let _ = self.finish_pane_move_drag(cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.pane_resize_drag.is_some() {
            if event.dragging() {
                if self.apply_pane_resize_drag(event.position) {
                    cx.notify();
                }
            } else if self.finish_pane_resize_drag(cx) {
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }

        // Track pane divider hover state for cursor feedback
        if !event.dragging() {
            let hit = self.pane_resize_hit_test(event.position, window);
            let next_hover = hit.map(|h| HoveredPaneDivider {
                pane_id: h.pane_id,
                axis: h.axis,
                edge: h.edge,
            });
            if self.hovered_pane_divider != next_hover {
                self.hovered_pane_divider = next_hover;
                cx.notify();
            }
        }

        if self.try_forward_mouse_move(event, cx) {
            return;
        }

        if !self.selection_dragging || !event.dragging() {
            if !Self::is_link_modifier(event.modifiers) {
                if self.clear_hovered_link() {
                    cx.notify();
                }
                return;
            }

            let hover_cell = self.position_to_cell(event.position, false);
            if let (Some(cell), Some(current)) = (hover_cell, self.hovered_link.as_ref())
                && current.contains(cell)
            {
                return;
            }

            let next = hover_cell.and_then(|cell| self.link_at_cell(cell));
            if self.hovered_link != next {
                self.hovered_link = next;
                cx.notify();
            }
        }

        // Selection drag updates are handled by the global mouse-move listener so drag
        // behavior remains continuous when the pointer exits the terminal bounds.
    }

    pub(in super::super) fn handle_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button == MouseButton::Left && self.finish_workspace_sidebar_resize_drag() {
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if event.button == MouseButton::Left && self.workspace_drag.is_some() {
            self.commit_workspace_drag(cx);
            cx.stop_propagation();
            return;
        }

        if event.button == MouseButton::Left && self.stop_terminal_scrollbar_track_hold() {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if event.button == MouseButton::Left && self.finish_terminal_scrollbar_drag(cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if event.button == MouseButton::Left && self.pane_move_drag.is_some() {
            let _ = self.finish_pane_move_drag(cx);
            cx.stop_propagation();
            return;
        }
        if event.button == MouseButton::Left && self.finish_pane_resize_drag(cx) {
            cx.stop_propagation();
            cx.notify();
            return;
        }

        if self.finish_pending_cursor_move_click(event.button, cx) {
            return;
        }

        if !Self::should_finish_selection_drag(event.button, self.selection_dragging) {
            return;
        }

        self.finish_selection_drag_at_position(Some(event.position), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_tab_drag_pointer_action_tracks_drag_until_release() {
        assert_eq!(
            TerminalView::global_tab_drag_pointer_action(true, true),
            GlobalTabDragPointerAction::UpdatePreview
        );
        assert_eq!(
            TerminalView::global_tab_drag_pointer_action(true, false),
            GlobalTabDragPointerAction::Commit
        );
        assert_eq!(
            TerminalView::global_tab_drag_pointer_action(false, true),
            GlobalTabDragPointerAction::None
        );
    }

    #[test]
    fn image_click_is_kept_but_drag_falls_back_to_text_selection() {
        assert_eq!(
            TerminalView::selection_release_action(false, true),
            SelectionReleaseAction::KeepImage
        );
        assert_eq!(
            TerminalView::selection_release_action(true, true),
            SelectionReleaseAction::FinishTextSelection
        );
        assert_eq!(
            TerminalView::selection_release_action(false, false),
            SelectionReleaseAction::MoveCursor
        );
    }

    #[test]
    fn selection_drag_autoscroll_lines_scrolls_up_when_pointer_above_top() {
        let lines =
            TerminalView::selection_drag_autoscroll_lines_from_bounds(90.0, 100.0, 300.0, 20.0);
        // Positive delta scrolls toward history (up).
        assert!(lines > 0);
    }

    #[test]
    fn selection_drag_autoscroll_lines_scrolls_down_when_pointer_below_bottom() {
        let lines =
            TerminalView::selection_drag_autoscroll_lines_from_bounds(330.0, 100.0, 300.0, 20.0);
        // Negative delta scrolls toward bottom (down).
        assert!(lines < 0);
    }

    #[test]
    fn selection_drag_autoscroll_lines_is_zero_inside_bounds() {
        let lines =
            TerminalView::selection_drag_autoscroll_lines_from_bounds(200.0, 100.0, 300.0, 20.0);
        assert_eq!(lines, 0);
    }

    #[test]
    fn selection_drag_autoscroll_lines_clamps_to_max_speed() {
        let up = TerminalView::selection_drag_autoscroll_lines_from_bounds(
            -10_000.0, 100.0, 300.0, 20.0,
        );
        let down =
            TerminalView::selection_drag_autoscroll_lines_from_bounds(10_000.0, 100.0, 300.0, 20.0);

        // up (pointer far above) -> positive, clamped to +MAX
        assert_eq!(up, SELECTION_DRAG_AUTOSCROLL_MAX_LINES);
        // down (pointer far below) -> negative, clamped to -MAX
        assert_eq!(down, -SELECTION_DRAG_AUTOSCROLL_MAX_LINES);
    }

    #[test]
    fn finish_selection_drag_only_for_left_button_with_active_drag() {
        assert!(TerminalView::should_finish_selection_drag(
            MouseButton::Left,
            true
        ));
        assert!(!TerminalView::should_finish_selection_drag(
            MouseButton::Left,
            false
        ));
        assert!(!TerminalView::should_finish_selection_drag(
            MouseButton::Right,
            true
        ));
    }

    #[test]
    fn cursor_move_input_for_click_target_moves_horizontally_only() {
        assert_eq!(
            TerminalView::cursor_move_input_for_click_target(
                CellPos { col: 2, row: 4 },
                CellPos { col: 5, row: 4 },
            ),
            Some(b"\x1b[C\x1b[C\x1b[C".to_vec())
        );
        assert_eq!(
            TerminalView::cursor_move_input_for_click_target(
                CellPos { col: 5, row: 4 },
                CellPos { col: 2, row: 4 },
            ),
            Some(b"\x1b[D\x1b[D\x1b[D".to_vec())
        );
        assert_eq!(
            TerminalView::cursor_move_input_for_click_target(
                CellPos { col: 5, row: 4 },
                CellPos { col: 2, row: 3 },
            ),
            None
        );
    }

    #[test]
    fn pending_cursor_move_only_turns_into_selection_after_cell_change() {
        let pending = PendingCursorMoveClick {
            pane_id: "%pane".to_string(),
            selection_start: SelectionPos { col: 2, line: 4 },
            start_cell: CellPos { col: 2, row: 4 },
            target: CellPos { col: 8, row: 4 },
        };

        assert!(!TerminalView::pending_cursor_move_starts_selection(
            &pending,
            "%pane",
            CellPos { col: 2, row: 4 },
        ));
        assert!(TerminalView::pending_cursor_move_starts_selection(
            &pending,
            "%pane",
            CellPos { col: 3, row: 4 },
        ));
        assert!(!TerminalView::pending_cursor_move_starts_selection(
            &pending,
            "%other",
            CellPos { col: 3, row: 4 },
        ));
    }

    #[test]
    fn cursor_move_preview_timer_matches_expected_budget() {
        assert_eq!(CURSOR_MOVE_PREVIEW_MS, 75);
    }

    #[test]
    fn click_cursor_move_allowed_state_works_without_prompt_markers() {
        assert!(TerminalView::click_cursor_move_allowed_for_state(
            RuntimeKind::Native,
            false,
            false,
            false,
        ));
        assert!(!TerminalView::click_cursor_move_allowed_for_state(
            RuntimeKind::Native,
            false,
            true,
            false,
        ));
        assert!(!TerminalView::click_cursor_move_allowed_for_state(
            RuntimeKind::Native,
            true,
            false,
            false,
        ));
        assert!(TerminalView::click_cursor_move_allowed_for_state(
            RuntimeKind::Tmux,
            false,
            true,
            false,
        ));
    }
}
