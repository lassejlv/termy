//! Visible grid and scrollback storage.

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use smol_str::SmolStrBuilder;
use unicode_width::UnicodeWidthChar;

use crate::{
    Cell, CellFlags, RowMoveDirection, TerminalMemoryStats,
    cell::CellTemplate,
    damage::Damage,
    search::{BufferPoint, BufferRange, SearchDirection, SearchOptions},
    selection::{SelectionPoint, SelectionRange},
};

pub(crate) type Row = Vec<Cell>;

const MAX_SPARE_ROWS: usize = 8;
const ROW_REUSE_CAPACITY_MULTIPLIER: usize = 4;
const ROW_REUSE_CAPACITY_SLACK: usize = 256;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Cursor {
    pub row: usize,
    pub column: usize,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Grid {
    columns: usize,
    rows: usize,
    cells: VecDeque<Row>,
    history: VecDeque<Row>,
    spare_rows: Vec<Row>,
    scrollback_limit: usize,
    history_enabled: bool,
    pub cursor: Cursor,
    saved_cursor: Cursor,
    scroll_top: usize,
    scroll_bottom: usize,
    tab_stops: Vec<bool>,
    wrap_pending: bool,
    display_offset: usize,
    viewport_revision: u64,
    pub damage: Damage,
}

impl Grid {
    pub(crate) fn new(
        columns: usize,
        rows: usize,
        scrollback_limit: usize,
        history_enabled: bool,
    ) -> Self {
        let columns = columns.max(2);
        let rows = rows.max(1);
        let mut damage = Damage::default();
        damage.set_row_count(rows);
        damage.full();
        Self {
            columns,
            rows,
            cells: (0..rows).map(|_| blank_row(columns)).collect(),
            history: VecDeque::new(),
            spare_rows: Vec::new(),
            scrollback_limit,
            history_enabled,
            cursor: Cursor::default(),
            saved_cursor: Cursor::default(),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            tab_stops: default_tab_stops(columns),
            wrap_pending: false,
            display_offset: 0,
            viewport_revision: 0,
            damage,
        }
    }

    pub(crate) const fn columns(&self) -> usize {
        self.columns
    }

    pub(crate) const fn rows(&self) -> usize {
        self.rows
    }

    pub(crate) const fn display_offset(&self) -> usize {
        self.display_offset
    }

    pub(crate) const fn viewport_revision(&self) -> u64 {
        self.viewport_revision
    }

    pub(crate) fn normalize_selection_column(&self, row: usize, column: usize) -> usize {
        let source = self.view_row(row);
        if column > 0
            && source
                .get(column)
                .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER))
            && source
                .get(column - 1)
                .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE))
        {
            column - 1
        } else {
            column
        }
    }

    pub(crate) fn word_selection_range(&self, point: SelectionPoint) -> SelectionRange {
        let row = self.view_row(point.row);
        let class = selection_cell_class(row, point.column);
        let mut start = point.column;
        while start > 0 && selection_cell_class(row, start - 1) == class {
            start -= 1;
        }
        let mut end = point.column;
        while end + 1 < self.columns && selection_cell_class(row, end + 1) == class {
            end += 1;
        }
        SelectionRange {
            start: SelectionPoint {
                column: start,
                row: point.row,
            },
            end: SelectionPoint {
                column: end,
                row: point.row,
            },
        }
    }

    pub(crate) fn hyperlink_at(&self, point: SelectionPoint) -> Option<Arc<String>> {
        self.view_row(point.row)
            .get(point.column)
            .and_then(|cell| cell.hyperlink.clone())
    }

    pub(crate) fn search(
        &mut self,
        query: &str,
        options: SearchOptions,
        origin: Option<BufferRange>,
    ) -> Option<(BufferRange, SelectionRange)> {
        let folded_query = fold_query(query, options.case_sensitive);
        if folded_query.is_empty() {
            return None;
        }
        let visible_top = self.history.len().saturating_sub(self.display_offset);
        let visible_bottom = visible_top.saturating_add(self.rows.saturating_sub(1));
        let anchor = origin.map_or_else(
            || match options.direction {
                SearchDirection::Forward => BufferPoint {
                    row: visible_top,
                    column: 0,
                },
                SearchDirection::Backward => BufferPoint {
                    row: visible_bottom,
                    column: usize::MAX,
                },
            },
            |range| range.start,
        );
        let mut first = None;
        let mut last = None;
        let mut backward_candidate = None;
        for row in 0..self.history.len().saturating_add(self.cells.len()) {
            for range in self.search_row(row, &folded_query, options.case_sensitive) {
                first.get_or_insert(range);
                last = Some(range);
                let eligible = if origin.is_some() {
                    match options.direction {
                        SearchDirection::Forward => range.start > anchor,
                        SearchDirection::Backward => range.start < anchor,
                    }
                } else {
                    match options.direction {
                        SearchDirection::Forward => range.start >= anchor,
                        SearchDirection::Backward => range.start <= anchor,
                    }
                };
                match options.direction {
                    SearchDirection::Forward if eligible => {
                        let visible = self.reveal_search_range(range);
                        return Some((range, visible));
                    }
                    SearchDirection::Backward if eligible => backward_candidate = Some(range),
                    SearchDirection::Forward | SearchDirection::Backward => {}
                }
            }
        }
        let range = match options.direction {
            SearchDirection::Backward => {
                backward_candidate.or_else(|| options.wrap.then_some(last).flatten())?
            }
            SearchDirection::Forward => options.wrap.then_some(first).flatten()?,
        };
        let visible = self.reveal_search_range(range);
        Some((range, visible))
    }

    fn search_row(
        &self,
        absolute_row: usize,
        query: &str,
        case_sensitive: bool,
    ) -> Vec<BufferRange> {
        let source = self.buffer_row(absolute_row);
        let mut line = String::new();
        let mut byte_columns = Vec::new();
        for (column, cell) in source.iter().take(self.columns).enumerate() {
            if cell.flags.contains(CellFlags::WIDE_SPACER) {
                continue;
            }
            let text = if cell.text.is_empty() {
                " "
            } else {
                cell.text.as_str()
            };
            for character in text.chars() {
                append_search_character(
                    &mut line,
                    &mut byte_columns,
                    character,
                    column,
                    case_sensitive,
                );
            }
        }
        let content_length = line.trim_end_matches(char::is_whitespace).len();
        line.truncate(content_length);
        byte_columns.truncate(content_length);
        if line.is_empty() {
            return Vec::new();
        }
        line.match_indices(query)
            .filter_map(|(start, matched)| {
                let end_byte = start.checked_add(matched.len())?.checked_sub(1)?;
                let start_column = *byte_columns.get(start)?;
                let mut end_column = *byte_columns.get(end_byte)?;
                if source
                    .get(end_column)
                    .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE))
                {
                    end_column = end_column.saturating_add(1).min(self.columns - 1);
                }
                Some(BufferRange {
                    start: BufferPoint {
                        row: absolute_row,
                        column: start_column,
                    },
                    end: BufferPoint {
                        row: absolute_row,
                        column: end_column,
                    },
                })
            })
            .collect()
    }

    fn buffer_row(&self, absolute_row: usize) -> &[Cell] {
        if absolute_row < self.history.len() {
            &self.history[absolute_row]
        } else {
            &self.cells[absolute_row - self.history.len()]
        }
    }

    fn reveal_search_range(&mut self, range: BufferRange) -> SelectionRange {
        let history_rows = self.history.len();
        let current_top = history_rows.saturating_sub(self.display_offset);
        let current_bottom = current_top.saturating_add(self.rows.saturating_sub(1));
        let visible_top = if (current_top..=current_bottom).contains(&range.start.row) {
            current_top
        } else if range.start.row >= history_rows {
            history_rows
        } else {
            range
                .start
                .row
                .saturating_sub(self.rows / 2)
                .min(history_rows)
        };
        let next_offset = history_rows.saturating_sub(visible_top);
        if next_offset != self.display_offset {
            let previous_offset = self.display_offset;
            self.display_offset = next_offset;
            self.record_display_scroll(previous_offset);
        }
        let row = range
            .start
            .row
            .saturating_sub(visible_top)
            .min(self.rows - 1);
        SelectionRange {
            start: SelectionPoint {
                column: range.start.column.min(self.columns - 1),
                row,
            },
            end: SelectionPoint {
                column: range.end.column.min(self.columns - 1),
                row,
            },
        }
    }

    pub(crate) fn reset(&mut self) {
        while let Some(row) = self.cells.pop_front() {
            self.recycle_row(row, self.columns);
        }
        for _ in 0..self.rows {
            let row = self.take_blank_row(self.columns, &CellTemplate::default());
            self.cells.push_back(row);
        }
        self.history.clear();
        self.history.shrink_to_fit();
        self.cursor = Cursor::default();
        self.saved_cursor = Cursor::default();
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
        self.tab_stops = default_tab_stops(self.columns);
        self.wrap_pending = false;
        self.display_offset = 0;
        self.damage.full();
    }

    pub(crate) fn resize(&mut self, columns: usize, rows: usize) {
        let columns = columns.max(2);
        let rows = rows.max(1);
        if columns == self.columns && rows == self.rows {
            return;
        }

        for row in &mut self.cells {
            row.resize_with(columns, Cell::default);
            row.truncate(columns);
            normalize_wide_row(row, &CellTemplate::default());
            compact_oversized_row(row, columns);
        }

        let old_rows = self.rows;
        if rows > old_rows {
            for _ in old_rows..rows {
                let row = self.take_blank_row(columns, &CellTemplate::default());
                self.cells.push_back(row);
            }
        } else {
            let removed = old_rows - rows;
            let removed_from_top = removed.min(self.cursor.row.saturating_sub(rows - 1));
            for _ in 0..removed_from_top {
                let row = self
                    .cells
                    .pop_front()
                    .expect("live grid always contains its configured rows");
                if self.history_enabled {
                    if let Some(reusable) = self.push_history(row) {
                        self.recycle_row(reusable, columns);
                    }
                } else {
                    self.recycle_row(row, columns);
                }
            }
            while self.cells.len() > rows {
                let row = self
                    .cells
                    .pop_back()
                    .expect("live grid has rows to remove while shrinking");
                self.recycle_row(row, columns);
            }
            self.cursor.row = self.cursor.row.saturating_sub(removed_from_top);
            self.saved_cursor.row = self.saved_cursor.row.saturating_sub(removed_from_top);
        }

        self.columns = columns;
        self.rows = rows;
        self.cursor.row = self.cursor.row.min(rows - 1);
        self.cursor.column = self.cursor.column.min(columns - 1);
        self.saved_cursor.row = self.saved_cursor.row.min(rows - 1);
        self.saved_cursor.column = self.saved_cursor.column.min(columns - 1);
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.tab_stops = default_tab_stops(columns);
        self.wrap_pending = false;
        self.display_offset = self.display_offset.min(self.history.len());
        self.damage.set_row_count(rows);
        self.damage.full();
    }

    pub(crate) fn print(&mut self, character: char, template: &CellTemplate, autowrap: bool) {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width == 0 {
            self.append_combining(character);
            return;
        }

        if self.wrap_pending && autowrap {
            self.cursor.column = 0;
            self.linefeed(template);
        }
        self.wrap_pending = false;

        if width == 2 && self.cursor.column + 1 >= self.columns {
            if autowrap {
                self.cursor.column = 0;
                self.linefeed(template);
            } else {
                return;
            }
        }

        let row = self.cursor.row;
        let column = self.cursor.column;
        let (dirty_start, mut dirty_end) = self.clear_wide_at(row, column, template);
        self.cells[row][column] = template.character(character, width);
        if width == 2 {
            let mut spacer = template.blank();
            spacer.flags.insert(CellFlags::WIDE_SPACER);
            self.cells[row][column + 1] = spacer;
            dirty_end = dirty_end.max(column + 2);
        }
        self.damage.span(row, dirty_start, dirty_end);

        let next = column + width;
        if next >= self.columns {
            self.cursor.column = self.columns - 1;
            self.wrap_pending = true;
        } else {
            self.cursor.column = next;
        }
    }

    fn append_combining(&mut self, character: char) {
        let (row, column) = if self.wrap_pending {
            (self.cursor.row, self.cursor.column)
        } else if self.cursor.column > 0 {
            (self.cursor.row, self.cursor.column - 1)
        } else {
            return;
        };
        let column = if self.cells[row][column]
            .flags
            .contains(CellFlags::WIDE_SPACER)
            && column > 0
        {
            column - 1
        } else {
            column
        };
        if !self.cells[row][column]
            .flags
            .contains(CellFlags::WIDE_SPACER)
        {
            let mut text = SmolStrBuilder::new();
            text.push_str(self.cells[row][column].text.as_str());
            text.push(character);
            self.cells[row][column].text = text.finish();
            self.damage.span(row, column, column + 1);
        }
    }

    fn clear_wide_at(
        &mut self,
        row: usize,
        column: usize,
        template: &CellTemplate,
    ) -> (usize, usize) {
        let mut start = column;
        let mut end = column + 1;
        if self.cells[row][column]
            .flags
            .contains(CellFlags::WIDE_SPACER)
            && column > 0
        {
            self.cells[row][column - 1] = template.blank();
            start = column - 1;
        }
        if self.cells[row][column].flags.contains(CellFlags::WIDE) && column + 1 < self.columns {
            self.cells[row][column + 1] = template.blank();
            end = column + 2;
        }
        (start, end)
    }

    pub(crate) fn linefeed(&mut self, template: &CellTemplate) {
        self.wrap_pending = false;
        if self.cursor.row == self.scroll_bottom {
            self.scroll_up(1, template);
        } else {
            self.cursor.row = (self.cursor.row + 1).min(self.rows - 1);
            self.damage.metadata();
        }
    }

    pub(crate) fn reverse_index(&mut self, template: &CellTemplate) {
        self.wrap_pending = false;
        if self.cursor.row == self.scroll_top {
            self.scroll_down(1, template);
        } else {
            self.cursor.row = self.cursor.row.saturating_sub(1);
            self.damage.metadata();
        }
    }

    pub(crate) fn carriage_return(&mut self) {
        self.cursor.column = 0;
        self.wrap_pending = false;
        self.damage.metadata();
    }

    pub(crate) fn backspace(&mut self) {
        self.cursor.column = self.cursor.column.saturating_sub(1);
        self.wrap_pending = false;
        self.damage.metadata();
    }

    pub(crate) fn tab(&mut self) {
        let next = ((self.cursor.column + 1)..self.columns)
            .find(|&column| self.tab_stops[column])
            .unwrap_or(self.columns - 1);
        self.cursor.column = next;
        self.wrap_pending = false;
        self.damage.metadata();
    }

    pub(crate) fn set_tab_stop(&mut self) {
        self.tab_stops[self.cursor.column] = true;
    }

    pub(crate) fn clear_tab_stop(&mut self, all: bool) {
        if all {
            self.tab_stops.fill(false);
        } else {
            self.tab_stops[self.cursor.column] = false;
        }
    }

    pub(crate) fn move_cursor_relative(&mut self, rows: isize, columns: isize, origin_mode: bool) {
        let minimum_row = if origin_mode { self.scroll_top } else { 0 };
        let maximum_row = if origin_mode {
            self.scroll_bottom
        } else {
            self.rows - 1
        };
        self.cursor.row = self
            .cursor
            .row
            .saturating_add_signed(rows)
            .clamp(minimum_row, maximum_row);
        self.cursor.column = self
            .cursor
            .column
            .saturating_add_signed(columns)
            .min(self.columns - 1);
        self.wrap_pending = false;
        self.damage.metadata();
    }

    pub(crate) fn set_cursor(&mut self, row: usize, column: usize, origin_mode: bool) {
        self.cursor.row = if origin_mode {
            self.scroll_top.saturating_add(row).min(self.scroll_bottom)
        } else {
            row.min(self.rows - 1)
        };
        self.cursor.column = column.min(self.columns - 1);
        self.wrap_pending = false;
        self.damage.metadata();
    }

    pub(crate) fn set_cursor_row(&mut self, row: usize, origin_mode: bool) {
        self.cursor.row = if origin_mode {
            self.scroll_top.saturating_add(row).min(self.scroll_bottom)
        } else {
            row.min(self.rows - 1)
        };
        self.wrap_pending = false;
        self.damage.metadata();
    }

    pub(crate) fn set_cursor_column(&mut self, column: usize) {
        self.cursor.column = column.min(self.columns - 1);
        self.wrap_pending = false;
        self.damage.metadata();
    }

    pub(crate) fn save_cursor(&mut self) {
        self.saved_cursor = self.cursor;
    }

    pub(crate) fn restore_cursor(&mut self) {
        self.cursor = self.saved_cursor;
        self.wrap_pending = false;
        self.damage.metadata();
    }

    pub(crate) fn erase_display(&mut self, mode: u16, template: &CellTemplate) {
        match mode {
            0 => {
                self.erase_line(0, template);
                for row in (self.cursor.row + 1)..self.rows {
                    self.cells[row].fill(template.blank());
                }
                if self.cursor.row + 1 < self.rows {
                    self.damage.rows(self.cursor.row + 1, self.rows - 1);
                }
            }
            1 => {
                self.erase_line(1, template);
                for row in 0..self.cursor.row {
                    self.cells[row].fill(template.blank());
                }
                if self.cursor.row > 0 {
                    self.damage.rows(0, self.cursor.row - 1);
                }
            }
            2 => {
                for row in &mut self.cells {
                    row.fill(template.blank());
                }
                self.damage.full();
            }
            3 => {
                let viewport_moved = self.display_offset != 0;
                self.history.clear();
                self.history.shrink_to_fit();
                self.display_offset = 0;
                if viewport_moved {
                    self.viewport_revision = self.viewport_revision.wrapping_add(1);
                }
                self.damage.full();
            }
            _ => {}
        }
    }

    pub(crate) fn erase_line(&mut self, mode: u16, template: &CellTemplate) {
        let range = match mode {
            0 => self.cursor.column..self.columns,
            1 => 0..self.cursor.column.saturating_add(1),
            2 => 0..self.columns,
            _ => return,
        };
        let (start, end) =
            wide_safe_erase_range(&self.cells[self.cursor.row], range.start, range.end);
        self.cells[self.cursor.row][start..end].fill(template.blank());
        self.damage.span(self.cursor.row, start, end);
    }

    pub(crate) fn erase_characters(&mut self, count: usize, template: &CellTemplate) {
        let end = (self.cursor.column + count.max(1)).min(self.columns);
        let (start, end) =
            wide_safe_erase_range(&self.cells[self.cursor.row], self.cursor.column, end);
        self.cells[self.cursor.row][start..end].fill(template.blank());
        self.damage.span(self.cursor.row, start, end);
    }

    pub(crate) fn insert_blank_characters(&mut self, count: usize, template: &CellTemplate) {
        let count = count.max(1).min(self.columns - self.cursor.column);
        let row = &mut self.cells[self.cursor.row];
        row[self.cursor.column..].rotate_right(count);
        row[self.cursor.column..self.cursor.column + count].fill(template.blank());
        normalize_wide_row(row, template);
        self.damage
            .span(self.cursor.row, self.cursor.column, self.columns);
    }

    pub(crate) fn delete_characters(&mut self, count: usize, template: &CellTemplate) {
        let count = count.max(1).min(self.columns - self.cursor.column);
        let row = &mut self.cells[self.cursor.row];
        row[self.cursor.column..].rotate_left(count);
        row[self.columns - count..].fill(template.blank());
        normalize_wide_row(row, template);
        self.damage
            .span(self.cursor.row, self.cursor.column, self.columns);
    }

    pub(crate) fn insert_lines(&mut self, count: usize, template: &CellTemplate) {
        if self.cursor.row < self.scroll_top || self.cursor.row > self.scroll_bottom {
            return;
        }
        let count = count.max(1).min(self.scroll_bottom - self.cursor.row + 1);
        for _ in 0..count {
            let removed = self
                .cells
                .remove(self.scroll_bottom)
                .expect("scroll bottom is always a live row");
            let blank = prepare_row(removed, self.columns, template);
            self.cells.insert(self.cursor.row, blank);
        }
        self.viewport_revision = self.viewport_revision.wrapping_add(1);
        self.record_scroll(
            self.cursor.row,
            self.scroll_bottom + 1,
            RowMoveDirection::Down,
            count,
        );
    }

    pub(crate) fn delete_lines(&mut self, count: usize, template: &CellTemplate) {
        if self.cursor.row < self.scroll_top || self.cursor.row > self.scroll_bottom {
            return;
        }
        let count = count.max(1).min(self.scroll_bottom - self.cursor.row + 1);
        for _ in 0..count {
            let removed = self
                .cells
                .remove(self.cursor.row)
                .expect("cursor row is always a live row");
            let blank = prepare_row(removed, self.columns, template);
            self.cells.insert(self.scroll_bottom, blank);
        }
        self.viewport_revision = self.viewport_revision.wrapping_add(1);
        self.record_scroll(
            self.cursor.row,
            self.scroll_bottom + 1,
            RowMoveDirection::Up,
            count,
        );
    }

    pub(crate) fn scroll_up(&mut self, count: usize, template: &CellTemplate) {
        let count = count.max(1).min(self.scroll_bottom - self.scroll_top + 1);
        let entire_screen = self.scroll_top == 0 && self.scroll_bottom == self.rows - 1;
        for _ in 0..count {
            let removed = if entire_screen {
                self.cells
                    .pop_front()
                    .expect("live grid always has a row to scroll")
            } else {
                self.cells
                    .remove(self.scroll_top)
                    .expect("scroll top is always a live row")
            };
            let reusable = if self.history_enabled && entire_screen {
                self.push_history(removed)
            } else {
                Some(removed)
            };
            let blank = if let Some(row) = reusable {
                prepare_row(row, self.columns, template)
            } else {
                self.take_blank_row(self.columns, template)
            };
            if entire_screen {
                self.cells.push_back(blank);
            } else {
                self.cells.insert(self.scroll_bottom, blank);
            }
        }
        self.viewport_revision = self.viewport_revision.wrapping_add(1);
        self.record_scroll(
            self.scroll_top,
            self.scroll_bottom + 1,
            RowMoveDirection::Up,
            count,
        );
    }

    pub(crate) fn scroll_down(&mut self, count: usize, template: &CellTemplate) {
        let count = count.max(1).min(self.scroll_bottom - self.scroll_top + 1);
        let entire_screen = self.scroll_top == 0 && self.scroll_bottom == self.rows - 1;
        for _ in 0..count {
            let removed = if entire_screen {
                self.cells
                    .pop_back()
                    .expect("live grid always has a row to scroll")
            } else {
                self.cells
                    .remove(self.scroll_bottom)
                    .expect("scroll bottom is always a live row")
            };
            let blank = prepare_row(removed, self.columns, template);
            if entire_screen {
                self.cells.push_front(blank);
            } else {
                self.cells.insert(self.scroll_top, blank);
            }
        }
        self.viewport_revision = self.viewport_revision.wrapping_add(1);
        self.record_scroll(
            self.scroll_top,
            self.scroll_bottom + 1,
            RowMoveDirection::Down,
            count,
        );
    }

    fn record_scroll(
        &mut self,
        start_row: usize,
        end_row: usize,
        direction: RowMoveDirection,
        count: usize,
    ) {
        if self.display_offset == 0 {
            self.damage.scroll(start_row, end_row, direction, count);
        } else {
            // Live-grid row indices do not map directly onto a scrollback viewport. Preserve a
            // simple correctness fallback until the view returns to the live screen.
            self.damage.full();
        }
    }

    fn push_history(&mut self, row: Row) -> Option<Row> {
        if self.scrollback_limit == 0 {
            return Some(row);
        }
        if self.display_offset > 0 {
            self.display_offset = (self.display_offset + 1).min(self.scrollback_limit);
        }
        let reusable = (self.history.len() == self.scrollback_limit)
            .then(|| self.history.pop_front())
            .flatten();
        self.history.push_back(row);
        self.display_offset = self.display_offset.min(self.history.len());
        reusable
    }

    pub(crate) fn set_scrollback_limit(&mut self, limit: usize) {
        let previous_limit = self.scrollback_limit;
        if limit == previous_limit {
            return;
        }
        self.scrollback_limit = limit;
        let remove = self.history.len().saturating_sub(limit);
        for _ in 0..remove {
            self.history.pop_front();
        }
        let history_was_visible = self.display_offset > 0 && remove > 0;
        self.display_offset = self.display_offset.min(self.history.len());
        if history_was_visible {
            self.viewport_revision = self.viewport_revision.wrapping_add(1);
            self.damage.full();
        }
        if limit < previous_limit {
            self.history.shrink_to_fit();
            self.spare_rows.clear();
            self.spare_rows.shrink_to_fit();
        }
    }

    pub(crate) fn add_memory_stats(&self, stats: &mut TerminalMemoryStats) {
        stats.live_rows = stats.live_rows.saturating_add(self.cells.len());
        stats.scrollback_rows = stats.scrollback_rows.saturating_add(self.history.len());
        stats.spare_rows = stats.spare_rows.saturating_add(self.spare_rows.len());
        stats.live_cell_capacity = stats
            .live_cell_capacity
            .saturating_add(self.cells.iter().map(Vec::capacity).sum::<usize>());
        stats.scrollback_cell_capacity = stats
            .scrollback_cell_capacity
            .saturating_add(self.history.iter().map(Vec::capacity).sum::<usize>());
        stats.spare_cell_capacity = stats
            .spare_cell_capacity
            .saturating_add(self.spare_rows.iter().map(Vec::capacity).sum::<usize>());
        stats.live_row_capacity = stats
            .live_row_capacity
            .saturating_add(self.cells.capacity());
        stats.scrollback_row_capacity = stats
            .scrollback_row_capacity
            .saturating_add(self.history.capacity());
        let (damage_rows, damage_snapshots) = self.damage.capacities();
        stats.damage_row_capacity = stats.damage_row_capacity.saturating_add(damage_rows);
        stats.damage_snapshot_capacity = stats
            .damage_snapshot_capacity
            .saturating_add(damage_snapshots);
    }

    fn take_blank_row(&mut self, columns: usize, template: &CellTemplate) -> Row {
        self.spare_rows.pop().map_or_else(
            || vec![template.blank(); columns],
            |row| prepare_row(row, columns, template),
        )
    }

    fn recycle_row(&mut self, mut row: Row, columns: usize) {
        if self.spare_rows.len() == MAX_SPARE_ROWS
            || row.capacity() > reusable_row_capacity(columns)
        {
            return;
        }
        row.clear();
        self.spare_rows.push(row);
    }

    pub(crate) fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        if top < bottom && bottom < self.rows {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        }
    }

    pub(crate) fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
    }

    pub(crate) fn scroll_display(&mut self, lines: isize) -> bool {
        let previous = self.display_offset;
        if lines > 0 {
            self.display_offset = self
                .display_offset
                .saturating_add(lines.unsigned_abs())
                .min(self.history.len());
        } else {
            self.display_offset = self.display_offset.saturating_sub(lines.unsigned_abs());
        }
        if self.display_offset != previous {
            self.record_display_scroll(previous);
        }
        self.display_offset != previous
    }

    pub(crate) fn scroll_to_bottom(&mut self) {
        if self.display_offset != 0 {
            let previous = self.display_offset;
            self.display_offset = 0;
            self.record_display_scroll(previous);
        }
    }

    fn record_display_scroll(&mut self, previous_offset: usize) {
        let count = self.display_offset.abs_diff(previous_offset);
        if count >= self.rows {
            self.damage.full();
            return;
        }
        let direction = if self.display_offset > previous_offset {
            RowMoveDirection::Down
        } else {
            RowMoveDirection::Up
        };
        self.damage.scroll(0, self.rows, direction, count);
    }

    pub(crate) fn view_row(&self, row: usize) -> &[Cell] {
        let start = self.history.len().saturating_sub(self.display_offset);
        let combined = start + row;
        if combined < self.history.len() {
            &self.history[combined]
        } else {
            let live = combined - self.history.len();
            &self.cells[live]
        }
    }

    pub(crate) fn copy_view_span(&self, row: usize, start: usize, end: usize) -> Vec<Cell> {
        let source = self.view_row(row);
        if source.len() == self.columns {
            return source[start..end].to_vec();
        }

        let mut resized = vec![Cell::default(); self.columns];
        let copied = source.len().min(self.columns);
        resized[..copied].clone_from_slice(&source[..copied]);
        normalize_wide_row(&mut resized, &CellTemplate::default());
        resized[start..end].to_vec()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectionCellClass {
    Whitespace,
    Word,
    Punctuation,
}

fn selection_cell_class(row: &[Cell], column: usize) -> SelectionCellClass {
    let mut column = column.min(row.len().saturating_sub(1));
    if column > 0
        && row[column].flags.contains(CellFlags::WIDE_SPACER)
        && row[column - 1].flags.contains(CellFlags::WIDE)
    {
        column -= 1;
    }
    let text = row[column].text.as_str();
    if text.chars().all(char::is_whitespace) {
        SelectionCellClass::Whitespace
    } else if text.chars().next().is_some_and(is_terminal_word_character) {
        SelectionCellClass::Word
    } else {
        SelectionCellClass::Punctuation
    }
}

fn is_terminal_word_character(character: char) -> bool {
    character.is_alphanumeric()
        || matches!(
            character,
            '_' | '-' | '.' | '/' | '~' | ':' | '@' | '%' | '+' | '=' | '?' | '&' | '#'
        )
}

fn fold_query(query: &str, case_sensitive: bool) -> String {
    let mut folded = String::with_capacity(query.len());
    for character in query.chars() {
        if case_sensitive {
            folded.push(character);
        } else {
            folded.extend(character.to_lowercase());
        }
    }
    folded
}

fn append_search_character(
    output: &mut String,
    byte_columns: &mut Vec<usize>,
    character: char,
    column: usize,
    case_sensitive: bool,
) {
    let start = output.len();
    if case_sensitive {
        output.push(character);
    } else {
        output.extend(character.to_lowercase());
    }
    byte_columns.resize(output.len(), column);
    debug_assert!(output.len() > start);
}

fn reusable_row_capacity(columns: usize) -> usize {
    columns
        .saturating_mul(ROW_REUSE_CAPACITY_MULTIPLIER)
        .max(columns.saturating_add(ROW_REUSE_CAPACITY_SLACK))
}

fn compact_oversized_row(row: &mut Row, columns: usize) {
    if row.capacity() <= reusable_row_capacity(columns) {
        return;
    }
    let mut compact = Vec::with_capacity(columns);
    compact.append(row);
    *row = compact;
}

fn prepare_row(mut row: Row, columns: usize, template: &CellTemplate) -> Row {
    if row.capacity() > reusable_row_capacity(columns) {
        return vec![template.blank(); columns];
    }
    row.clear();
    row.resize(columns, template.blank());
    row
}

fn blank_row(columns: usize) -> Row {
    vec![Cell::default(); columns]
}

fn default_tab_stops(columns: usize) -> Vec<bool> {
    (0..columns)
        .map(|column| column != 0 && column % 8 == 0)
        .collect()
}

fn wide_safe_erase_range(row: &[Cell], start: usize, end: usize) -> (usize, usize) {
    let mut start = start.min(row.len());
    let mut end = end.min(row.len());
    if start < row.len() && start > 0 && row[start].flags.contains(CellFlags::WIDE_SPACER) {
        start -= 1;
    }
    if end > 0 && end < row.len() && row[end - 1].flags.contains(CellFlags::WIDE) {
        end += 1;
    }
    (start, end)
}

fn normalize_wide_row(row: &mut [Cell], template: &CellTemplate) {
    let mut column = 0;
    while column < row.len() {
        if row[column].flags.contains(CellFlags::WIDE) {
            if column + 1 < row.len() && row[column + 1].flags.contains(CellFlags::WIDE_SPACER) {
                column += 2;
                continue;
            }
            row[column] = template.blank();
        } else if row[column].flags.contains(CellFlags::WIDE_SPACER)
            && (column == 0 || !row[column - 1].flags.contains(CellFlags::WIDE))
        {
            row[column] = template.blank();
        }
        column += 1;
    }
}

#[cfg(test)]
mod tests {
    use smol_str::SmolStr;

    use crate::{TerminalMemoryStats, cell::CellTemplate};

    use super::{Grid, blank_row, reusable_row_capacity};

    fn mark_row(grid: &mut Grid, row: usize, text: &str) {
        grid.cells[row][0].text = SmolStr::new(text);
    }

    fn row_marks(grid: &Grid) -> String {
        grid.cells.iter().map(|row| row[0].text.as_str()).collect()
    }

    #[test]
    fn resize_does_not_rewrite_scrollback_rows() {
        let mut grid = Grid::new(160, 50, 10_000, true);
        for _ in 0..10_000 {
            let _ = grid.push_history(blank_row(160));
        }
        let history_cells = grid.history.iter().map(Vec::len).sum::<usize>();

        grid.resize(159, 50);

        assert_eq!(
            grid.history.iter().map(Vec::len).sum::<usize>(),
            history_cells,
            "resizing the viewport must not synchronously rewrite scrollback"
        );
    }

    #[test]
    fn scrollback_is_resized_lazily_when_it_enters_the_viewport() {
        let mut grid = Grid::new(4, 2, 10, true);
        let mut history_row = blank_row(4);
        for (cell, text) in history_row.iter_mut().zip(["a", "b", "c", "d"]) {
            cell.text = SmolStr::new(text);
        }
        let _ = grid.push_history(history_row);
        grid.display_offset = 1;

        grid.resize(2, 2);
        let narrow = grid.copy_view_span(0, 0, 2);
        assert_eq!(
            narrow
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>(),
            "ab"
        );

        grid.resize(6, 2);
        let wide = grid.copy_view_span(0, 0, 6);
        assert_eq!(
            wide.iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>(),
            "abcd  ",
            "a temporary narrow viewport must not destroy wider scrollback"
        );
    }

    #[test]
    fn full_screen_scrolls_rotate_and_reuse_live_row_allocations() {
        let template = CellTemplate::default();
        let mut grid = Grid::new(4, 3, 0, false);
        mark_row(&mut grid, 0, "a");
        mark_row(&mut grid, 1, "b");
        mark_row(&mut grid, 2, "c");
        let first_allocation = grid.cells[0].as_ptr();

        grid.scroll_up(1, &template);
        assert_eq!(row_marks(&grid), "bc ");
        assert_eq!(grid.cells[2].as_ptr(), first_allocation);

        mark_row(&mut grid, 2, "d");
        let last_allocation = grid.cells[2].as_ptr();
        grid.scroll_down(1, &template);
        assert_eq!(row_marks(&grid), " bc");
        assert_eq!(grid.cells[0].as_ptr(), last_allocation);
    }

    #[test]
    fn insert_and_delete_lines_reuse_the_removed_row() {
        let template = CellTemplate::default();
        let mut grid = Grid::new(4, 4, 0, false);
        for (row, mark) in ["a", "b", "c", "d"].into_iter().enumerate() {
            mark_row(&mut grid, row, mark);
        }
        grid.cursor.row = 1;
        let bottom_allocation = grid.cells[3].as_ptr();

        grid.insert_lines(1, &template);
        assert_eq!(row_marks(&grid), "a bc");
        assert_eq!(grid.cells[1].as_ptr(), bottom_allocation);

        let inserted_allocation = grid.cells[1].as_ptr();
        grid.delete_lines(1, &template);
        assert_eq!(row_marks(&grid), "abc ");
        assert_eq!(grid.cells[3].as_ptr(), inserted_allocation);
    }

    #[test]
    fn full_history_reuses_the_evicted_row_for_the_live_blank() {
        let template = CellTemplate::default();
        let mut grid = Grid::new(4, 2, 1, true);
        mark_row(&mut grid, 0, "a");
        mark_row(&mut grid, 1, "b");
        grid.scroll_up(1, &template);
        let evicted_allocation = grid.history[0].as_ptr();

        mark_row(&mut grid, 1, "c");
        grid.scroll_up(1, &template);

        assert_eq!(grid.history[0][0].text, "b");
        assert_eq!(row_marks(&grid), "c ");
        assert_eq!(grid.cells[1].as_ptr(), evicted_allocation);
    }

    #[test]
    fn narrowing_a_very_wide_grid_drops_oversized_row_capacity() {
        let mut grid = Grid::new(4096, 3, 0, false);
        let before = grid.cells.iter().map(Vec::capacity).sum::<usize>();

        grid.resize(8, 3);

        let limit = reusable_row_capacity(8);
        assert!(grid.cells.iter().all(|row| row.capacity() <= limit));
        assert!(grid.cells.iter().map(Vec::capacity).sum::<usize>() < before / 8);
    }

    #[test]
    fn shrinking_scrollback_drops_rows_and_container_capacity() {
        let mut grid = Grid::new(8, 2, 64, true);
        for _ in 0..64 {
            let _ = grid.push_history(blank_row(8));
        }
        let mut before = TerminalMemoryStats::default();
        grid.add_memory_stats(&mut before);

        grid.set_scrollback_limit(3);

        let mut after = TerminalMemoryStats::default();
        grid.add_memory_stats(&mut after);
        assert_eq!(after.scrollback_rows, 3);
        assert!(after.scrollback_cell_capacity < before.scrollback_cell_capacity);
        assert!(after.scrollback_row_capacity < before.scrollback_row_capacity);
    }
}
