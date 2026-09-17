use super::*;

impl Grid {
    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        let cols = usize::from(cols.clamp(1, MAX_GRID_DIMENSION));
        let rows = usize::from(rows.clamp(1, MAX_GRID_DIMENSION));
        if self.primary.cols == cols && self.primary.rows == rows {
            return;
        }

        if self.primary.cols != cols {
            // Match Alacritty's resize order: visible rows are adjusted before
            // the column reflow decides which source rows survive widening.
            self.resize_primary_rows(rows);
            self.reflow_primary(cols, rows);
            if let Some(alternate) = &mut self.alternate {
                alternate.resize(cols, rows, false);
            }
            // Kitty placements stay anchored to terminal lines across a width change.
            // Rendering clips placements outside the resized viewport, matching the
            // native engine and allowing them to reappear after a later expansion.
        } else {
            self.resize_primary_rows(rows);
            if let Some(alternate) = &mut self.alternate {
                let alternate_old_rows = alternate.rows;
                let alternate_cursor_row = alternate.cursor_row;
                alternate.resize(cols, rows, false);
                if rows < alternate_old_rows {
                    let count = alternate_cursor_row.saturating_add(1).saturating_sub(rows);
                    if count > 0 {
                        self.effects.push_back(GridEffect::ScrollUp {
                            alternate: true,
                            top: 0,
                            bottom: alternate_old_rows.saturating_sub(1),
                            count,
                            full_screen_region: true,
                            recorded_history: false,
                            history_before: 0,
                            history_after: 0,
                        });
                    }
                }
            }
        }
        self.display_offset = self.display_offset.min(self.history.len());
        let old_tab_count = self.tab_stops.len();
        self.tab_stops.resize(cols, false);
        for col in old_tab_count..cols {
            if col != 0 && col % 8 == 0 {
                self.tab_stops[col] = true;
            }
        }
        self.damage.resize(rows);
    }

    pub(super) fn resize_primary_rows(&mut self, rows: usize) {
        let old_rows = self.primary.rows;
        if rows == old_rows {
            return;
        }
        let cols = self.primary.cols;
        if rows > old_rows {
            let added = rows - old_rows;
            let pulled = added.min(self.history.len());
            let pulled_rows = self.history.split_off(self.history.len() - pulled);
            let mut cells = Vec::with_capacity(rows);
            cells.extend(pulled_rows.into_iter().map(HistoryRow::into_dense));
            cells.append(&mut self.primary.cells);
            cells.resize_with(rows, || vec![Cell::default(); cols]);
            self.primary.cells = cells;
            self.primary.rebuild_row_extents();
            self.primary.rows = rows;
            self.primary.cursor_row = self
                .primary
                .cursor_row
                .saturating_add(pulled)
                .min(rows.saturating_sub(1));
            self.primary.saved_cursor_row = self
                .primary
                .saved_cursor_row
                .saturating_add(pulled)
                .min(rows.saturating_sub(1));
            self.primary.scroll_top = 0;
            self.primary.scroll_bottom = rows.saturating_sub(1);
            self.display_offset = self
                .display_offset
                .saturating_sub(added)
                .min(self.history.len());
            return;
        }

        let required_scrolling = self
            .primary
            .cursor_row
            .saturating_add(1)
            .saturating_sub(rows);
        let history_before = self.history.len();
        let removed = self
            .primary
            .cells
            .drain(..required_scrolling)
            .collect::<Vec<_>>();
        let removed_extents = self
            .primary
            .row_extents
            .drain(..required_scrolling)
            .collect::<Vec<_>>();
        for (row, extent) in removed.into_iter().zip(removed_extents) {
            let _ = self.push_history(row, extent);
        }
        let history_after = self.history.len();
        self.primary.cells.truncate(rows);
        self.primary.row_extents.truncate(rows);
        self.primary.rows = rows;
        self.primary.cursor_row = self.primary.cursor_row.min(rows.saturating_sub(1));
        self.primary.saved_cursor_row = self.primary.saved_cursor_row.min(rows.saturating_sub(1));
        self.primary.scroll_top = 0;
        self.primary.scroll_bottom = rows.saturating_sub(1);
        if required_scrolling > 0 {
            self.effects.push_back(GridEffect::ScrollUp {
                alternate: false,
                top: 0,
                bottom: old_rows.saturating_sub(1),
                count: required_scrolling,
                full_screen_region: true,
                recorded_history: self.history_limit > 0,
                history_before,
                history_after,
            });
        }
    }

    pub(super) fn reflow_primary(&mut self, cols: usize, rows: usize) {
        let old_cols = self.primary.cols;
        let display_offset = self.display_offset;
        let cursor_row = self.primary.cursor_row;
        let cursor_col = self.primary.cursor_col;
        let old_wrap_pending = self.primary.wrap_pending;
        let last_meaningful_row = if self.resize_anchor_suppressed_after_clear {
            self.primary.rows.saturating_sub(1)
        } else {
            (0..self.primary.rows)
                .rev()
                .find(|&row| {
                    self.primary
                        .row(row)
                        .iter()
                        .any(|cell| !cell_is_empty_for_viewport(cell))
                })
                .unwrap_or(0)
                .max(cursor_row)
        };

        let old_history = std::mem::take(&mut self.history);
        let history_rows = old_history.len();
        let old_history = old_history
            .into_iter()
            .map(HistoryRow::into_dense)
            .collect::<VecDeque<_>>();
        let old_physical_rows = history_rows.saturating_add(self.primary.rows);
        let all_screen_rows = std::mem::take(&mut self.primary.cells);
        let reflowed_display_offset = if display_offset == 0 {
            0
        } else if cols > old_cols {
            display_offset_after_growing_columns(
                old_history
                    .iter()
                    .map(Vec::as_slice)
                    .chain(all_screen_rows.iter().map(Vec::as_slice)),
                old_physical_rows,
                history_rows.saturating_add(cursor_row),
                cols,
                display_offset,
            )
        } else {
            display_offset_after_shrinking_columns(
                old_history
                    .iter()
                    .map(Vec::as_slice)
                    .chain(all_screen_rows.iter().map(Vec::as_slice)),
                old_physical_rows,
                self.primary.rows,
                cursor_row,
                cursor_col,
                old_wrap_pending,
                cols,
                display_offset,
            )
        };
        let screen_rows = all_screen_rows
            .into_iter()
            .take(last_meaningful_row + 1)
            .collect::<Vec<_>>();
        let mut output = VecDeque::with_capacity(history_rows.saturating_add(screen_rows.len()));
        let mut logical = Vec::with_capacity(old_cols.saturating_mul(2));
        let mut logical_cursor = None;
        let mut mapped_cursor = None;
        let mut growing_row = None;

        for (physical_row, row) in old_history.into_iter().chain(screen_rows).enumerate() {
            let continues_logical_line = !logical.is_empty();
            let wrapped = row.last().is_some_and(Cell::wrapped);
            let is_cursor_row = physical_row == history_rows.saturating_add(cursor_row);
            let growing_transition = (cols > old_cols)
                .then(|| advance_growing_row(&mut growing_row, &row, cols))
                .flatten();
            let retained_cursor_continuation = is_cursor_row
                .then(|| {
                    growing_transition.and_then(|transition| {
                        retained_cursor_continuation(
                            transition,
                            rows,
                            cursor_row,
                            cursor_col,
                            old_wrap_pending,
                            cols,
                        )
                    })
                })
                .flatten();
            let meaningful_used = if wrapped {
                old_cols
            } else {
                row.iter()
                    .rposition(|cell| !cell_is_empty_for_viewport(cell))
                    .map_or(0, |col| col + 1)
            };
            let mut used = meaningful_used;
            if is_cursor_row && retained_cursor_continuation.is_none() {
                let cursor_starts_logical_line = logical.is_empty();
                let cursor_on_empty_continuation = !logical.is_empty()
                    && meaningful_used == 0
                    && cursor_col == 0
                    && !old_wrap_pending;
                let cursor_offset = cursor_col.saturating_add(usize::from(old_wrap_pending));
                // Alacritty trims default trailing cells from a soft-wrapped continuation when
                // the cursor lands exactly on the new boundary. Otherwise the empty cells still
                // participate in cursor reflow, including when the cursor fits inside the row.
                let cursor_offset =
                    if cols < old_cols && !logical.is_empty() && cursor_offset == cols {
                        cursor_offset.min(meaningful_used)
                    } else {
                        cursor_offset
                    };
                used = used.max(cursor_offset);
                let retained_offset = row
                    .iter()
                    .take(cursor_offset.min(used))
                    .filter(|cell| !cell.leading_wide_spacer())
                    .count();
                logical_cursor = Some((
                    logical.len().saturating_add(retained_offset),
                    cursor_on_empty_continuation,
                    cursor_starts_logical_line,
                ));
            }
            let used = retained_cursor_continuation.map_or(used, |continuation| {
                // Alacritty pulls the complete physical prefix, including default
                // cells, before retaining the clear cursor continuation.
                continuation.remainder_start
            });
            for mut cell in row.iter().copied().take(used) {
                if cell.leading_wide_spacer() {
                    continue;
                }
                cell.set_wrapped(false);
                logical.push(cell);
            }
            if let Some(continuation) = retained_cursor_continuation {
                let (_, final_row, final_col) = flush_reflow_line(
                    &mut logical,
                    cols,
                    &mut output,
                    logical_cursor.take(),
                    false,
                    false,
                );
                if let Some(previous) = output.get_mut(final_row) {
                    let wrap_col = final_col.saturating_sub(1).min(cols.saturating_sub(1));
                    previous[wrap_col].set_wrapped(true);
                }
                let mut continuation_row = row[continuation.remainder_start..].to_vec();
                continuation_row.resize(cols, Cell::default());
                let continuation_row_index = output.len();
                output.push_back(continuation_row);
                mapped_cursor = Some((
                    continuation_row_index,
                    continuation.target_col,
                    continuation.target_wrap_pending,
                ));
                continue;
            }
            if !wrapped {
                let trailing_empty_continuation = continues_logical_line && meaningful_used == 0;
                let (cursor, _, _) = flush_reflow_line(
                    &mut logical,
                    cols,
                    &mut output,
                    logical_cursor.take(),
                    cols < old_cols,
                    trailing_empty_continuation,
                );
                if cursor.is_some() {
                    mapped_cursor = cursor;
                }
            }
        }
        if !logical.is_empty() {
            let (cursor, _, _) = flush_reflow_line(
                &mut logical,
                cols,
                &mut output,
                logical_cursor.take(),
                cols < old_cols,
                false,
            );
            if cursor.is_some() {
                mapped_cursor = cursor;
            }
        }

        if output.is_empty() {
            output.push_back(vec![Cell::default(); cols]);
        }
        let mut mapped_cursor = mapped_cursor.unwrap_or((output.len().saturating_sub(1), 0, false));
        let split = output.len().saturating_sub(rows);
        if mapped_cursor.0 < split {
            // Reflowing content below the cursor can push the cursor's logical
            // source line into history. Alacritty retains that newer content
            // and clamps the live cursor to the top of the viewport instead of
            // truncating rows below it.
            mapped_cursor.0 = split;
        }
        let mapped_display_offset = reflowed_display_offset;

        self.history = output.drain(..split).map(HistoryRow::from_dense).collect();
        while self.history.len() > self.history_limit {
            self.pop_history_front();
        }
        self.display_offset = mapped_display_offset.min(self.history.len());
        let screen_row_count = output.len().min(rows);
        let cursor_row = mapped_cursor
            .0
            .saturating_sub(split)
            .min(rows.saturating_sub(1));
        mapped_cursor.1 = mapped_cursor.1.min(cols.saturating_sub(1));

        let mut cells = output.into_iter().take(rows).collect::<Vec<_>>();
        cells.resize_with(rows, || vec![Cell::default(); cols]);
        self.primary.cols = cols;
        self.primary.rows = rows;
        self.primary.cells = cells;
        self.primary.rebuild_row_extents();
        self.primary.cursor_row = cursor_row.min(screen_row_count.max(1).saturating_sub(1));
        self.primary.cursor_col = mapped_cursor.1;
        self.primary.saved_cursor_row = self.primary.saved_cursor_row.min(rows.saturating_sub(1));
        self.primary.saved_cursor_col = self.primary.saved_cursor_col.min(cols.saturating_sub(1));
        self.primary.wrap_pending = mapped_cursor.2;
        self.primary.scroll_top = 0;
        self.primary.scroll_bottom = rows.saturating_sub(1);
    }

    pub(crate) fn set_cell_metrics(&mut self, width: f32, height: f32) {
        self.cell_width = finite_metric(width);
        self.cell_height = finite_metric(height);
    }

    pub(crate) fn text_area_size_pixels(&self) -> (u32, u32) {
        let width = (self.cols() as f32 * self.cell_width)
            .round()
            .clamp(1.0, u32::MAX as f32) as u32;
        let height = (self.rows() as f32 * self.cell_height)
            .round()
            .clamp(1.0, u32::MAX as f32) as u32;
        (height, width)
    }

    pub(crate) fn set_history_limit(&mut self, limit: usize) {
        let history_before = self.history.len();
        self.history_limit = limit.min(MAX_SCROLLBACK_LINES);
        while self.history.len() > self.history_limit {
            self.pop_history_front();
        }
        let dropped = history_before.saturating_sub(self.history.len());
        if dropped > 0 {
            self.history.shrink_to_fit();
            self.release_unused_metadata();
            self.effects
                .push_back(GridEffect::RebaseHistory { dropped });
        }
        self.display_offset = self.display_offset.min(self.history.len());
        self.damage.mark_full();
    }

    pub(crate) fn set_default_cursor_style(&mut self, style: CursorStyle) {
        if !self.cursor_style_explicit {
            self.cursor_style = style;
            self.cursor_shape_status = cursor_shape_status(style);
        }
        self.default_cursor_style = style;
        self.damage.mark_full();
    }

    pub(crate) fn scroll_display(&mut self, delta_lines: i32) -> bool {
        if self.alternate_active || delta_lines == 0 {
            return false;
        }
        let old = self.display_offset;
        if delta_lines > 0 {
            self.display_offset = self
                .display_offset
                .saturating_add(delta_lines as usize)
                .min(self.history.len());
        } else {
            self.display_offset = self
                .display_offset
                .saturating_sub(delta_lines.unsigned_abs() as usize);
        }
        if old != self.display_offset {
            self.damage.mark_full();
            true
        } else {
            false
        }
    }

    pub(crate) fn scroll_to_bottom(&mut self) -> bool {
        if self.display_offset == 0 {
            return false;
        }
        self.display_offset = 0;
        self.damage.mark_full();
        true
    }

    pub(crate) fn clear_history(&mut self) -> bool {
        if self.alternate_active {
            return false;
        }
        self.resize_anchor_suppressed_after_clear = false;
        if self.history.is_empty() && self.display_offset == 0 {
            return false;
        }
        let dropped = self.history.len();
        while self.pop_history_front().is_some() {}
        self.display_offset = 0;
        if dropped > 0 {
            self.history.shrink_to_fit();
            self.release_unused_metadata();
            self.effects
                .push_back(GridEffect::RebaseHistory { dropped });
        }
        self.damage.mark_full();
        true
    }

    pub(crate) fn resize_anchor_suppressed_after_clear(&self) -> bool {
        self.resize_anchor_suppressed_after_clear
    }

    pub(crate) fn observe_live_output_for_resize_anchor(&mut self, was_suppressed: bool) {
        if !was_suppressed || self.alternate_active {
            return;
        }
        let cursor_row = self.primary.cursor_row;
        if cursor_row.saturating_add(1) >= self.primary.rows {
            self.resize_anchor_suppressed_after_clear = false;
        }
    }

    pub(crate) fn pop_effect(&mut self) -> Option<GridEffect> {
        self.effects.pop_front()
    }

    pub(crate) fn take_damage(&mut self) -> DamageSnapshot {
        let display_offset = self.display_offset();
        self.damage.take(display_offset)
    }

    pub(crate) fn render_damage_snapshot(&self) -> (DamageSnapshot, Vec<ScrollDamage>) {
        let display_offset = self.display_offset();
        self.damage.render_snapshot(display_offset)
    }

    pub(crate) fn take_render_damage(&mut self) -> (DamageSnapshot, Vec<ScrollDamage>) {
        let display_offset = self.display_offset();
        self.damage.take_render(display_offset)
    }

    pub(crate) fn clear_damage(&mut self) {
        self.damage.clear();
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        let offset = self.display_offset();
        let rows = self.rows();
        let cols = self.cols();
        let mut cells = Vec::with_capacity(cols.saturating_mul(rows));
        for row in 0..rows {
            let term_line = row as i32 - offset as i32;
            if let Some(line) = self.line(term_line) {
                line.append_to(&mut cells, cols);
            }
        }
        Snapshot {
            cols: cols as u16,
            rows: rows as u16,
            cells,
            cursor: self.cursor_state(),
            display_offset: offset,
            history_size: self.history_size(),
        }
    }

    #[inline]
    pub(crate) fn visit_viewport_cells(
        &self,
        mut visitor: impl FnMut(usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> ViewportMetadata {
        let offset = self.display_offset();
        let rows = self.rows();
        let cols = self.cols();
        if offset == 0 {
            for (row, cells) in self.active().cells.iter().take(rows).enumerate() {
                let term_line = row as i32;
                for (col, cell) in cells.iter().take(cols).enumerate() {
                    visitor(0, term_line, col, cell, self.combining_text(cell));
                }
            }
        } else {
            for row in 0..rows {
                let term_line = row as i32 - offset as i32;
                if let Some(line) = self.line(term_line) {
                    line.for_each(|col, cell| {
                        if col < cols {
                            visitor(offset, term_line, col, cell, self.combining_text(cell));
                        }
                    });
                }
            }
        }
        ViewportMetadata {
            cols: cols as u16,
            rows: rows as u16,
            cursor: self.cursor_state(),
            display_offset: offset,
            history_size: self.history_size(),
        }
    }

    pub(crate) fn with_viewport_cell<R>(
        &self,
        row: usize,
        col: usize,
        f: impl FnOnce(usize, i32, &Cell) -> R,
    ) -> Option<R> {
        if row >= self.rows() || col >= self.cols() {
            return None;
        }
        let offset = self.display_offset();
        let term_line = row as i32 - offset as i32;
        let cell = self.line(term_line)?.get(col)?;
        Some(f(offset, term_line, &cell))
    }

    pub(crate) fn hyperlink_at(&self, row: usize, col: usize) -> Option<Hyperlink> {
        if row >= self.rows() || col >= self.cols() {
            return None;
        }
        let line_index = row as i32 - self.display_offset() as i32;
        let line = self.line(line_index)?;
        let id = self.cell_hyperlink_id(&line.get(col)?)?;
        let mut start_col = col;
        while start_col > 0
            && line
                .get(start_col - 1)
                .and_then(|cell| self.cell_hyperlink_id(&cell))
                == Some(id)
        {
            start_col -= 1;
        }
        let mut end_col = col;
        while end_col + 1 < line.len()
            && line
                .get(end_col + 1)
                .and_then(|cell| self.cell_hyperlink_id(&cell))
                == Some(id)
        {
            end_col += 1;
        }
        Some(Hyperlink {
            start_col,
            end_col,
            target: self.hyperlinks.get(&id.get())?.target.clone(),
        })
    }

    pub(crate) fn link_candidate(&self, row: usize, col: usize) -> Option<LinkCandidate> {
        let columns = self.cols();
        let screen_lines = self.rows();
        if row >= screen_lines || col >= columns || columns == 0 {
            return None;
        }

        let display_offset = self.display_offset();
        let display_offset_i32 = i32::try_from(display_offset).ok()?;
        let hovered = GridPosition {
            line: i32::try_from(row).ok()?.checked_sub(display_offset_i32)?,
            col,
        };
        let bounds = self.line_bounds();
        let hovered_cell = self.cell_at_position(hovered)?;

        if let Some(hyperlink_id) = self.cell_hyperlink_id(&hovered_cell) {
            let target = self.hyperlinks.get(&hyperlink_id.get())?.target.clone();
            let mut start = hovered;
            while let Some(previous) = self.previous_wrapped_position(start, bounds.0, columns) {
                if self
                    .cell_at_position(previous)
                    .and_then(|cell| self.cell_hyperlink_id(&cell))
                    == Some(hyperlink_id)
                {
                    start = previous;
                } else {
                    break;
                }
            }

            let mut end = hovered;
            while let Some(next) = self.next_wrapped_position(end, bounds.1, columns) {
                if self
                    .cell_at_position(next)
                    .and_then(|cell| self.cell_hyperlink_id(&cell))
                    == Some(hyperlink_id)
                {
                    end = next;
                } else {
                    break;
                }
            }

            return viewport_link_from_grid_range(
                start,
                end,
                target,
                display_offset,
                screen_lines,
                columns,
            )
            .map(LinkCandidate::Resolved);
        }

        let hovered_character = self.link_character(hovered)?;
        if hovered_character.is_whitespace() {
            return None;
        }

        let mut start = hovered;
        while let Some(previous) = self.previous_wrapped_position(start, bounds.0, columns) {
            if self.link_character(previous)?.is_whitespace() {
                break;
            }
            start = previous;
        }

        let line_distance = usize::try_from(hovered.line.checked_sub(start.line)?).ok()?;
        let hovered_index = line_distance
            .checked_mul(columns)?
            .checked_add(hovered.col)?
            .checked_sub(start.col)?;
        let mut characters = Vec::with_capacity(columns.min(128));
        let mut cursor = start;
        loop {
            let character = self.link_character(cursor)?;
            if character.is_whitespace() {
                break;
            }
            characters.push(character);
            let Some(next) = self.next_wrapped_position(cursor, bounds.1, columns) else {
                break;
            };
            cursor = next;
        }
        if hovered_index >= characters.len() {
            return None;
        }

        Some(LinkCandidate::Text(TextLinkCandidate {
            characters,
            hovered_index,
            start,
            display_offset,
            screen_lines,
            columns,
        }))
    }

    pub(super) fn cell_at_position(&self, position: GridPosition) -> Option<Cell> {
        self.line(position.line)?.get(position.col)
    }

    pub(super) fn previous_wrapped_position(
        &self,
        position: GridPosition,
        min_line: i32,
        columns: usize,
    ) -> Option<GridPosition> {
        if position.col > 0 {
            return Some(GridPosition {
                line: position.line,
                col: position.col - 1,
            });
        }
        let previous_line = position.line.checked_sub(1)?;
        if previous_line < min_line {
            return None;
        }
        let previous = GridPosition {
            line: previous_line,
            col: columns.checked_sub(1)?,
        };
        self.cell_at_position(previous)?
            .wrapped()
            .then_some(previous)
    }

    pub(super) fn next_wrapped_position(
        &self,
        position: GridPosition,
        max_line: i32,
        columns: usize,
    ) -> Option<GridPosition> {
        if position.col + 1 < columns {
            return Some(GridPosition {
                line: position.line,
                col: position.col + 1,
            });
        }
        if position.line >= max_line || !self.cell_at_position(position)?.wrapped() {
            return None;
        }
        Some(GridPosition {
            line: position.line.checked_add(1)?,
            col: 0,
        })
    }

    pub(super) fn link_character(&self, position: GridPosition) -> Option<char> {
        let cell = self.cell_at_position(position)?;
        Some(
            if cell.wide_spacer()
                || cell.leading_wide_spacer()
                || cell.attributes.hidden()
                || cell.character == '\0'
                || cell.character.is_control()
            {
                ' '
            } else {
                cell.character
            },
        )
    }

    pub(crate) fn for_each_viewport_range(
        &self,
        ranges: impl IntoIterator<Item = (usize, usize, usize)>,
        mut visitor: impl FnMut(usize, usize, i32, usize, &Cell, Option<Combining<'_>>),
    ) -> usize {
        let offset = self.display_offset();
        let rows = self.rows();
        let cols = self.cols();
        for (row, left, right) in ranges {
            if row >= rows || left >= cols || left > right {
                continue;
            }
            let term_line = row as i32 - offset as i32;
            let Some(line) = self.line(term_line) else {
                continue;
            };
            let right = right.min(cols.saturating_sub(1));
            for col in left..=right {
                if let Some(cell) = line.get(col) {
                    visitor(
                        row,
                        offset,
                        term_line,
                        col,
                        &cell,
                        self.combining_text(&cell),
                    );
                }
            }
        }
        offset
    }

    pub(crate) fn line_bounds(&self) -> (i32, i32) {
        if self.alternate_active {
            (0, self.rows().saturating_sub(1) as i32)
        } else {
            (
                -(self.history.len() as i32),
                self.rows().saturating_sub(1) as i32,
            )
        }
    }

    pub(crate) fn line(&self, line: i32) -> Option<RowView<'_>> {
        if self.alternate_active {
            let row = usize::try_from(line).ok()?;
            let alternate = self
                .alternate
                .as_ref()
                .expect("the active alternate screen is initialized");
            return (row < alternate.rows).then(|| RowView::Dense(alternate.row(row)));
        }
        if line < 0 {
            let index = self.history.len() as i64 + i64::from(line);
            return usize::try_from(index)
                .ok()
                .and_then(|index| self.history.get(index))
                .map(RowView::History);
        }
        let row = usize::try_from(line).ok()?;
        (row < self.primary.rows).then(|| RowView::Dense(self.primary.row(row)))
    }

    pub(crate) fn for_each_line_cell(
        &self,
        line: i32,
        mut visitor: impl FnMut(usize, &Cell, Option<Combining<'_>>),
    ) -> bool {
        let Some(cells) = self.line(line) else {
            return false;
        };
        cells.for_each(|col, cell| visitor(col, cell, self.combining_text(cell)));
        true
    }

    pub(crate) fn for_each_line_cell_range(
        &self,
        requested_first: i32,
        requested_last: i32,
        mut visitor: impl FnMut(i32, usize, &Cell, Option<Combining<'_>>),
    ) {
        if requested_first > requested_last {
            return;
        }
        let (first_line, last_line) = self.line_bounds();
        let first = requested_first.max(first_line);
        let last = requested_last.min(last_line);
        if first > last {
            return;
        }
        for line in first..=last {
            let Some(cells) = self.line(line) else {
                continue;
            };
            cells.for_each(|col, cell| visitor(line, col, cell, self.combining_text(cell)));
        }
    }

    #[inline]
    pub(super) fn combining_text(&self, cell: &Cell) -> Option<Combining<'_>> {
        if !cell.has_combining() {
            return None;
        }
        let id = cell.metadata_id?;
        inline_combining_character(id)
            .map(Combining::Character)
            .or_else(|| {
                self.extras
                    .get(&id)
                    .map(|extra| Combining::Text(extra.combining.as_str()))
            })
    }

    pub(super) fn cell_hyperlink_id(&self, cell: &Cell) -> Option<NonZeroU32> {
        cell_hyperlink_id(&self.extras, cell)
    }
}

pub(super) fn viewport_link_from_grid_range(
    start: GridPosition,
    end: GridPosition,
    target: String,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
) -> Option<ViewportLink> {
    let display_offset = i32::try_from(display_offset).ok()?;
    let viewport_min_line = -display_offset;
    let viewport_max_line = i32::try_from(screen_lines)
        .ok()?
        .checked_sub(1)?
        .checked_sub(display_offset)?;
    let visible_start_line = start.line.max(viewport_min_line);
    let visible_end_line = end.line.min(viewport_max_line);
    if visible_start_line > visible_end_line {
        return None;
    }

    Some(ViewportLink {
        start_row: usize::try_from(visible_start_line.checked_add(display_offset)?).ok()?,
        start_col: if start.line < visible_start_line {
            0
        } else {
            start.col
        },
        end_row: usize::try_from(visible_end_line.checked_add(display_offset)?).ok()?,
        end_col: if end.line > visible_end_line {
            columns.checked_sub(1)?
        } else {
            end.col
        },
        target,
    })
}

pub(super) fn inline_combining_metadata_id(character: char) -> NonZeroU32 {
    NonZeroU32::new(character as u32 + 1).expect("Unicode scalar values produce nonzero IDs")
}

pub(super) fn inline_combining_character(id: NonZeroU32) -> Option<char> {
    (id.get() & METADATA_TAG_MASK == 0)
        .then(|| char::from_u32(id.get() - 1))
        .flatten()
}

pub(super) fn inline_hyperlink_metadata_id(id: NonZeroU32) -> NonZeroU32 {
    debug_assert!(id.get() <= METADATA_VALUE_MASK);
    NonZeroU32::new(INLINE_HYPERLINK_TAG | id.get())
        .expect("tagged hyperlink metadata IDs are nonzero")
}

pub(super) fn inline_hyperlink_id(id: NonZeroU32) -> Option<NonZeroU32> {
    (id.get() & METADATA_TAG_MASK == INLINE_HYPERLINK_TAG)
        .then(|| NonZeroU32::new(id.get() & METADATA_VALUE_MASK))
        .flatten()
}

pub(super) fn pooled_extra_metadata_id(raw: u32) -> NonZeroU32 {
    debug_assert!((1..=METADATA_VALUE_MASK).contains(&raw));
    NonZeroU32::new(POOLED_EXTRA_TAG | raw).expect("tagged pooled metadata IDs are nonzero")
}

pub(super) fn is_pooled_extra_id(id: NonZeroU32) -> bool {
    id.get() & METADATA_TAG_MASK == POOLED_EXTRA_TAG
}

pub(super) fn cell_hyperlink_id(
    extras: &HashMap<NonZeroU32, CellExtra>,
    cell: &Cell,
) -> Option<NonZeroU32> {
    if !cell.has_hyperlink() {
        return None;
    }
    let metadata_id = cell.metadata_id?;
    inline_hyperlink_id(metadata_id).or_else(|| {
        extras
            .get(&metadata_id)
            .and_then(|extra| extra.hyperlink_id)
    })
}

pub(super) fn default_tab_stops(cols: usize) -> Vec<bool> {
    (0..cols).map(|col| col % 8 == 0).collect()
}

fn finite_metric(value: f32) -> f32 {
    if value.is_finite() {
        value.max(1.0)
    } else {
        1.0
    }
}

pub(super) fn cell_is_empty_for_viewport(cell: &Cell) -> bool {
    matches!(cell.character, ' ' | '\t')
        && !cell.has_combining()
        && cell.foreground == Color::Default
        && cell.background == Color::Default
        && !cell.attributes.inverse()
        && !cell.attributes.underline()
        && !cell.attributes.strikethrough()
        && !cell.wide_spacer()
        && !cell.leading_wide_spacer()
        && !cell.wrapped()
}

#[derive(Clone, Copy)]
struct GrowingRowState {
    len: usize,
    wrapped: bool,
    leading_wide_spacer: bool,
}

#[derive(Clone, Copy)]
struct GrowingRowTransition {
    remainder_start: usize,
    num_wrapped: usize,
    remainder_is_clear: bool,
}

#[derive(Clone, Copy)]
struct RetainedCursorContinuation {
    remainder_start: usize,
    target_col: usize,
    target_wrap_pending: bool,
}

#[derive(Clone, Copy)]
struct ShrinkingCellState(u8);

impl Default for ShrinkingCellState {
    fn default() -> Self {
        Self(Self::BASE_EMPTY)
    }
}

impl ShrinkingCellState {
    const BASE_EMPTY: u8 = 1 << 0;
    const WRAPPED: u8 = 1 << 1;
    const WIDE_CHARACTER: u8 = 1 << 2;
    const WIDE_SPACER: u8 = 1 << 3;
    const LEADING_WIDE_SPACER: u8 = 1 << 4;

    fn from_cell(row: &[Cell], index: usize) -> Self {
        let cell = row[index];
        let base_empty = matches!(cell.character, ' ' | '\t')
            && !cell.has_combining()
            && cell.foreground == Color::Default
            && cell.background == Color::Default
            && !cell.attributes.inverse()
            && !cell.attributes.underline()
            && !cell.attributes.strikethrough();
        let wide_character =
            index + 1 < row.len() && row[index + 1].wide_spacer() && !cell.wide_spacer();
        let mut state = Self::default();
        state.set(Self::BASE_EMPTY, base_empty);
        state.set(Self::WRAPPED, cell.wrapped());
        state.set(Self::WIDE_CHARACTER, wide_character);
        state.set(Self::WIDE_SPACER, cell.wide_spacer());
        state.set(Self::LEADING_WIDE_SPACER, cell.leading_wide_spacer());
        state
    }

    fn leading_wide_spacer() -> Self {
        let mut state = Self::default();
        state.set(Self::BASE_EMPTY, true);
        state.set(Self::LEADING_WIDE_SPACER, true);
        state
    }

    fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    fn set(&mut self, flag: u8, enabled: bool) {
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }

    fn is_empty(self) -> bool {
        self.contains(Self::BASE_EMPTY)
            && !self.contains(Self::WRAPPED)
            && !self.contains(Self::WIDE_SPACER)
            && !self.contains(Self::LEADING_WIDE_SPACER)
    }

    fn wrapped(self) -> bool {
        self.contains(Self::WRAPPED)
    }

    fn set_wrapped(&mut self, enabled: bool) {
        self.set(Self::WRAPPED, enabled);
    }

    fn wide_character(self) -> bool {
        self.contains(Self::WIDE_CHARACTER)
    }

    fn leading(self) -> bool {
        self.contains(Self::LEADING_WIDE_SPACER)
    }
}

impl GrowingRowState {
    fn from_suffix(row: &[Cell], start: usize) -> Option<Self> {
        let suffix = row.get(start..)?;
        let last = suffix.last()?;
        Some(Self {
            len: suffix.len(),
            wrapped: last.wrapped(),
            leading_wide_spacer: last.leading_wide_spacer(),
        })
    }
}

fn advance_growing_row(
    retained: &mut Option<GrowingRowState>,
    row: &[Cell],
    columns: usize,
) -> Option<GrowingRowTransition> {
    let Some(mut previous) = *retained else {
        *retained = GrowingRowState::from_suffix(row, 0);
        return None;
    };

    if !previous.wrapped || previous.len == 0 || previous.len >= columns {
        *retained = GrowingRowState::from_suffix(row, 0);
        return None;
    }

    if previous.leading_wide_spacer {
        previous.len = previous.len.saturating_sub(1);
    }
    let available = columns.saturating_sub(previous.len);
    let split_len = row.len().min(available);
    debug_assert!(split_len > 0);
    let wide_character_hits_edge =
        split_len < row.len() && row[split_len].wide_spacer() && !row[split_len - 1].wide_spacer();
    let remainder_start = split_len.saturating_sub(usize::from(wide_character_hits_edge));
    let num_wrapped = available.saturating_sub(usize::from(wide_character_hits_edge));

    previous.len = previous.len.saturating_add(split_len);
    if wide_character_hits_edge {
        previous.wrapped = false;
        previous.leading_wide_spacer = true;
    } else {
        let appended = &row[split_len - 1];
        previous.wrapped = appended.wrapped();
        previous.leading_wide_spacer = appended.leading_wide_spacer();
    }

    let remainder_is_clear = row[remainder_start..]
        .iter()
        .all(cell_is_empty_for_viewport);
    if remainder_is_clear {
        *retained = Some(previous);
    } else {
        *retained = GrowingRowState::from_suffix(row, remainder_start);
    }

    Some(GrowingRowTransition {
        remainder_start,
        num_wrapped,
        remainder_is_clear,
    })
}

fn retained_cursor_continuation(
    transition: GrowingRowTransition,
    rows: usize,
    cursor_row: usize,
    cursor_col: usize,
    wrap_pending: bool,
    columns: usize,
) -> Option<RetainedCursorContinuation> {
    if !transition.remainder_is_clear {
        return None;
    }

    let effective_col = cursor_col.saturating_add(usize::from(wrap_pending));
    let (mut target_row, mut target_col) = subtract_cursor_point(
        rows,
        columns,
        cursor_row,
        effective_col,
        transition.num_wrapped,
    );
    let target_wrap_pending = target_col == 0;
    if target_wrap_pending {
        (target_row, target_col) = subtract_cursor_point(rows, columns, target_row, target_col, 1);
    }
    (target_row == cursor_row).then_some(RetainedCursorContinuation {
        remainder_start: transition.remainder_start,
        target_col,
        target_wrap_pending,
    })
}

fn subtract_cursor_point(
    rows: usize,
    columns: usize,
    row: usize,
    col: usize,
    amount: usize,
) -> (usize, usize) {
    let line_changes = amount
        .saturating_add(columns.saturating_sub(1))
        .saturating_sub(col)
        / columns;
    if line_changes > row {
        return (0, 0);
    }

    let target_row = row - line_changes;
    let target_col = (columns + col - amount % columns) % columns;
    (
        target_row.min(rows.saturating_sub(1)),
        target_col.min(columns.saturating_sub(1)),
    )
}

/// Mirror Alacritty's `grow_columns` viewport rotation without copying rows.
///
/// Widening decrements `display_offset` only when an old non-cursor source row
/// below the viewport is completely absorbed into the preceding wrapped row.
/// Mapping a logical cell boundary is insufficient because a partially
/// absorbed row is still retained and therefore still counts toward the
/// viewport offset.
fn display_offset_after_growing_columns<'a>(
    rows: impl Iterator<Item = &'a [Cell]>,
    total_rows: usize,
    cursor_physical_row: usize,
    columns: usize,
    mut display_offset: usize,
) -> usize {
    let mut retained = None;

    for (physical_row, row) in rows.enumerate() {
        let buffer_line = total_rows.saturating_sub(physical_row + 1);
        let is_cursor_row = physical_row == cursor_physical_row;
        let Some(transition) = advance_growing_row(&mut retained, row, columns) else {
            continue;
        };
        if transition.remainder_is_clear {
            // Alacritty handles the live cursor before its clear-row removal
            // branch, so absorbing that source row never rotates the viewport.
            if !is_cursor_row && buffer_line < display_offset {
                display_offset = display_offset.saturating_sub(1);
            }
        }
    }

    display_offset
}

/// Mirror Alacritty's `shrink_columns` display-offset accounting using compact
/// one-byte cell state and three reusable buffers. This avoids cloning Tmon's
/// full cells just to determine how many physical rows are created below a
/// scrolled viewport.
fn display_offset_after_shrinking_columns<'a>(
    rows: impl Iterator<Item = &'a [Cell]>,
    total_rows: usize,
    visible_rows: usize,
    cursor_row: usize,
    mut cursor_col: usize,
    wrap_pending: bool,
    columns: usize,
    mut display_offset: usize,
) -> usize {
    if wrap_pending {
        cursor_col = cursor_col.saturating_add(1);
    }

    let mut cursor_row = cursor_row;
    let mut current = Vec::new();
    let mut wrapped = Vec::new();
    let mut buffered = Vec::new();

    for (physical_row, row) in rows.enumerate() {
        let buffer_line = total_rows.saturating_sub(physical_row + 1);
        current.clear();
        if !buffered.is_empty() {
            let cursor_buffer_line = visible_rows.saturating_sub(cursor_row + 1);
            if buffer_line == cursor_buffer_line {
                cursor_col = cursor_col.saturating_add(buffered.len());
            }
            current.append(&mut buffered);
        }
        current.extend(
            row.iter()
                .enumerate()
                .map(|(index, _)| ShrinkingCellState::from_cell(row, index)),
        );

        loop {
            wrapped.clear();
            if current.len() > columns {
                wrapped.extend_from_slice(&current[columns..]);
                let occupied = wrapped
                    .iter()
                    .rposition(|cell| !cell.is_empty())
                    .map_or(0, |index| index + 1);
                wrapped.truncate(occupied);
                current.truncate(columns);
            }

            if wrapped.is_empty() {
                let cursor_buffer_line = visible_rows.saturating_sub(cursor_row + 1);
                if buffer_line != cursor_buffer_line || cursor_col <= columns {
                    break;
                }
            }

            if columns > 1 && current.len() >= columns && current[columns - 1].wide_character() {
                let wide_character = std::mem::replace(
                    &mut current[columns - 1],
                    ShrinkingCellState::leading_wide_spacer(),
                );
                wrapped.insert(0, wide_character);
            }

            if wrapped.last().is_some_and(|cell| cell.leading()) {
                if wrapped.len() == 1 {
                    if let Some(last) = current.last_mut() {
                        last.set_wrapped(true);
                    }
                    break;
                }
                let new_last = wrapped.len() - 2;
                wrapped[new_last].set_wrapped(true);
                wrapped.pop();
            }

            if let Some(last) = current.last_mut() {
                last.set_wrapped(true);
            }
            if buffer_line >= 1
                && wrapped.len() < columns
                && wrapped.last().is_some_and(|cell| cell.wrapped())
            {
                if let Some(last) = wrapped.last_mut() {
                    last.set_wrapped(false);
                }
                std::mem::swap(&mut buffered, &mut wrapped);
                break;
            }

            let cursor_buffer_line = visible_rows.saturating_sub(cursor_row + 1);
            if (buffer_line == cursor_buffer_line && cursor_col < columns)
                || buffer_line < cursor_buffer_line
            {
                cursor_row = cursor_row.saturating_sub(1);
            }
            if buffer_line == cursor_buffer_line && cursor_col >= columns {
                cursor_col -= columns;
            }

            if wrapped.len() < columns {
                wrapped.resize(columns, ShrinkingCellState::default());
            }
            std::mem::swap(&mut current, &mut wrapped);
            if buffer_line < display_offset {
                display_offset = display_offset.saturating_add(1);
            }
        }
    }

    display_offset
}

fn flush_reflow_line(
    logical: &mut Vec<Cell>,
    cols: usize,
    output: &mut VecDeque<Vec<Cell>>,
    cursor_offset: Option<(usize, bool, bool)>,
    shrinking: bool,
    trailing_empty_continuation: bool,
) -> (Option<(usize, usize, bool)>, usize, usize) {
    let first_output_row = output.len();
    if logical.is_empty() {
        output.push_back(vec![Cell::default(); cols]);
        return (
            cursor_offset.map(|_| (first_output_row, 0, false)),
            first_output_row,
            0,
        );
    }

    let mut cursor = None;
    let mut index = 0usize;
    let logical_len = logical.len();
    let mut final_row = first_output_row;
    let mut final_col = 0usize;
    let mut wide_wrap_before_cursor = false;
    while index < logical.len() {
        let row_index = output.len();
        let mut row = vec![Cell::default(); cols];
        let mut col = 0usize;
        while col < cols && index < logical.len() {
            let wide_character_hits_edge = cols > 1
                && col + 1 == cols
                && index + 1 < logical.len()
                && logical[index + 1].wide_spacer()
                && !logical[index].wide_spacer();
            if wide_character_hits_edge {
                wide_wrap_before_cursor |=
                    cursor_offset.is_some_and(|(offset, _, _)| offset > index);
                let mut placeholder = logical[index + 1];
                placeholder.set_wide_spacer(false);
                placeholder.set_leading_wide_spacer(true);
                row[col] = placeholder;
                break;
            }
            let physical_cursor_reflow = shrinking
                && wide_wrap_before_cursor
                && cursor_offset.is_some_and(|(_, _, starts_logical)| starts_logical);
            if !physical_cursor_reflow
                && cursor_offset.is_some_and(|(offset, _, _)| offset == index)
            {
                cursor = Some((row_index, col, false));
            }
            row[col] = logical[index];
            index += 1;
            col += 1;
        }
        if index < logical.len() {
            row[cols.saturating_sub(1)].set_wrapped(true);
        }
        final_row = row_index;
        final_col = col;
        output.push_back(row);
    }
    let preserved_empty_continuation =
        !shrinking && trailing_empty_continuation && final_col == cols;
    if preserved_empty_continuation {
        if let Some(row) = output.back_mut() {
            row[cols.saturating_sub(1)].set_wrapped(true);
        }
        output.push_back(vec![Cell::default(); cols]);
    }
    if cursor.is_none()
        && (shrinking
            && wide_wrap_before_cursor
            && cursor_offset.is_some_and(|(_, _, starts_logical)| starts_logical)
            || cursor_offset.is_some_and(|(offset, _, _)| offset == logical_len))
    {
        cursor = if shrinking
            && wide_wrap_before_cursor
            && cursor_offset.is_some_and(|(_, _, starts_logical)| starts_logical)
        {
            let (offset, force_continuation, _) =
                cursor_offset.expect("cursor offset was checked above");
            let row_offset = offset / cols;
            let col = offset % cols;
            if col == 0 && offset > 0 && !force_continuation {
                Some((
                    first_output_row + row_offset.saturating_sub(1),
                    cols.saturating_sub(1),
                    true,
                ))
            } else {
                let row = first_output_row + row_offset;
                if row == output.len() {
                    if let Some(previous) = output.back_mut() {
                        previous[cols.saturating_sub(1)].set_wrapped(true);
                    }
                    output.push_back(vec![Cell::default(); cols]);
                }
                Some((row, col, false))
            }
        } else if preserved_empty_continuation && cursor_offset.is_some_and(|(_, force, _)| force) {
            Some((final_row + 1, 0, false))
        } else if final_col == cols && cursor_offset.is_some_and(|(_, force, _)| force) {
            if let Some(row) = output.back_mut() {
                row[cols.saturating_sub(1)].set_wrapped(true);
            }
            let row = output.len();
            output.push_back(vec![Cell::default(); cols]);
            Some((row, 0, false))
        } else if final_col == cols {
            Some((final_row, cols.saturating_sub(1), true))
        } else {
            Some((final_row, final_col, false))
        };
    }
    logical.clear();
    (cursor, final_row, final_col)
}
