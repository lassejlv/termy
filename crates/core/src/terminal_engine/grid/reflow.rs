//! Width-change reflow: rewraps logical lines into a bounded output window.
use super::super::types::{CellExtra, ExtraSlot};
use super::history::history_heap;
use super::{Cell, Grid, Ring, Row, Size, print};
use std::sync::Arc;

/// Keep only the rows a resized viewport and its bounded history can retain.
/// Row positions remain absolute while old rows are evicted, so cursor and
/// viewport anchors can be remapped without materializing the entire result.
struct ReflowWindow {
    rows: Ring<Row>,
    // Bytes charged to `rows`, as though `charge_cols` wide, and the bytes
    // they take (see `history.rs`), and the oldest row not yet packed for
    // the budget.
    bytes: usize,
    charge_cols: usize,
    heap: usize,
    byte_limit: usize,
    next_pack: usize,
    spare: Vec<Row>,
    size: Size,
    limit: usize,
    first: usize,
    produced: usize,
    cursor: Option<(usize, usize, bool)>,
    boundary: Option<usize>,
    viewport: Option<usize>,
    // Where each tracked row landed, by its index in `Grid::tracked_rows`,
    // and the tracked rows of the logical line being joined: the index of
    // each one's first cell, and its index in `pins`.
    pins: Vec<Option<usize>>,
    logical_pins: Vec<(usize, usize)>,
    #[cfg(test)]
    row_allocations: usize,
}

impl ReflowWindow {
    /// Rows are charged as though the narrower of `size` and `old_cols`
    /// wide, so widening evicts no history the old width kept.
    fn new(size: Size, old_cols: usize, history_limit: usize, history_byte_limit: usize) -> Self {
        Self {
            rows: Ring::new(),
            bytes: 0,
            charge_cols: old_cols.min(size.cols),
            heap: 0,
            byte_limit: history_byte_limit,
            next_pack: 0,
            spare: Vec::new(),
            size,
            limit: history_limit.saturating_add(size.rows),
            first: 0,
            produced: 0,
            cursor: None,
            boundary: None,
            viewport: None,
            pins: Vec::new(),
            logical_pins: Vec::new(),
            #[cfg(test)]
            row_allocations: 0,
        }
    }

    // Enough recycled rows to cover the joins of a widening reflow.
    const MAX_SPARE: usize = 8;

    fn new_row(&mut self) -> Row {
        if let Some(mut row) = self.spare.pop() {
            // A row recycled from a much wider screen must not keep its width.
            if row.cells.capacity() > 2 * self.size.cols {
                row.cells = Vec::new();
            }
            row.clear(self.size.cols, &Cell::default());
            row
        } else {
            #[cfg(test)]
            {
                self.row_allocations += 1;
            }
            Row::new(self.size.cols, &Cell::default())
        }
    }

    /// Keep a consumed source row's buffer for a later output row.
    fn recycle(&mut self, mut row: Row) {
        if self.spare.len() < Self::MAX_SPARE {
            // Its remaining cells are not known to be blank.
            row.cells.clear();
            row.extras.clear();
            row.occupied = 0;
            row.clear_background = Cell::default().style.background;
            self.spare.push(row);
        }
    }

    fn push(&mut self, mut row: Row) {
        let index = self.produced;
        self.produced += 1;
        if self
            .cursor
            .is_some_and(|(cursor, _, _)| index >= cursor + self.size.rows)
        {
            // Resize discards rows below the cursor's final visible screen.
            // Continue counting them, but reuse their buffers.
            self.recycle(row);
            return;
        }
        if self.rows.len() == self.limit {
            self.evict();
        }
        row.measure_extras();
        self.bytes += self.charge(&row);
        self.heap += history_heap(&row);
        self.rows.push_back(row);
        self.fit_byte_budget();
    }

    /// Rewrapped rows are dense at the new width. Pack the oldest rows above
    /// the last screenful while they take more than the history byte budget,
    /// then evict rows while their charge exceeds it. Packing changes no
    /// charge, so which rows are evicted doesn't depend on which rows were
    /// packed before the resize. Each row is packed at most once.
    fn fit_byte_budget(&mut self) {
        let budget = self.byte_limit.saturating_add(
            self.size.rows * (size_of::<Row>() + self.size.cols * size_of::<Cell>()),
        );
        let packable = self.rows.len().saturating_sub(self.size.rows);
        while self.heap > budget && self.next_pack < packable {
            let row = &mut self.rows[self.next_pack];
            let before = row.heap_bytes();
            row.compact();
            self.heap = self.heap - before + row.heap_bytes();
            self.next_pack += 1;
        }
        while self.bytes > budget && self.rows.len() > self.size.rows {
            self.evict();
        }
    }

    fn charge(&self, row: &Row) -> usize {
        size_of::<Row>() + row.charged_bytes_at(self.charge_cols)
    }

    fn evict(&mut self) {
        if let Some(evicted) = self.rows.pop_front() {
            self.bytes -= self.charge(&evicted);
            self.heap -= history_heap(&evicted);
            self.next_pack = self.next_pack.saturating_sub(1);
            self.recycle(evicted);
        }
        self.first += 1;
    }
}

impl Grid {
    /// Follow these rows through the next width change. They count from the
    /// oldest history row, ascending and without repeats, as a placed
    /// image's rows are known to the engine.
    pub(in crate::terminal_engine) fn track_rows(&mut self, rows: Vec<usize>) {
        self.tracked_rows = rows;
        self.tracked_moves = None;
    }

    /// Each tracked row and where a width change moved it, counted from the
    /// oldest history row it kept (negative when the row was evicted). None
    /// when the width did not change, so nothing moved.
    pub(in crate::terminal_engine) fn take_tracked_rows(&mut self) -> Option<Vec<(usize, i64)>> {
        self.tracked_rows.clear();
        self.tracked_moves.take()
    }

    pub(super) fn reflow_primary(&mut self, size: Size) {
        let tracked = std::mem::take(&mut self.tracked_rows);
        let mut next_tracked = 0;
        let old_history = self.history.len();
        let cursor_source = old_history + self.primary.cursor.row;
        let cursor_col = self.primary.cursor.col;
        let cursor_pending = self.primary.pending_wrap;
        let viewport_source =
            (self.display_offset != 0).then_some(old_history - self.display_offset);
        // Unused bottom rows do not become logical history during reflow.
        while self.primary.rows.len() > self.primary.cursor.row + 1
            && self
                .primary
                .rows
                .back()
                .is_some_and(|row| row.content_len() == 0)
        {
            self.primary.rows.pop_back();
        }
        let source = self.history.drain(..).chain(self.primary.rows.drain(..));
        let mut output = ReflowWindow::new(
            size,
            self.size.cols,
            self.history_limit,
            self.history_byte_limit,
        );
        let mut logical = Vec::new();
        // The metadata of `logical`'s cells, by index.
        let mut logical_extras = Vec::new();
        let mut logical_cursor = None;
        let mut logical_boundary = None;
        let mut logical_viewport = None;
        output.pins = vec![None; tracked.len()];
        // Rows printed on a one-column screen hold narrowed wide glyphs that
        // only the logical-line path restores.
        let in_place = self.size.cols > 1;
        let mut joining = false;
        for (index, mut row) in source.enumerate() {
            let pin = (tracked.get(next_tracked) == Some(&index)).then(|| {
                next_tracked += 1;
                next_tracked - 1
            });
            if in_place && !joining && index != cursor_source && row.reflow_in_place(size.cols) {
                if index == old_history {
                    output.boundary = Some(output.produced);
                }
                if let Some(pin) = pin {
                    output.pins[pin] = Some(output.produced);
                }
                if Some(index) == viewport_source {
                    output.viewport = Some(output.produced);
                }
                output.push(row);
                continue;
            }
            joining = row.wrapped;
            row.make_dense();
            if index == old_history {
                logical_boundary = Some(logical.len());
            }
            if Some(index) == viewport_source {
                logical_viewport = Some(logical.len());
            }
            if let Some(pin) = pin {
                output.logical_pins.push((logical.len(), pin));
            }
            if index == cursor_source {
                let removed = row.cells[..cursor_col]
                    .iter()
                    .filter(|cell| cell.flags & Cell::LEADING_WIDE_SPACER != 0)
                    .count();
                logical_cursor =
                    Some(logical.len() + cursor_col - removed + usize::from(cursor_pending));
            }
            let required = if index == cursor_source {
                cursor_col + usize::from(cursor_pending)
            } else {
                0
            };
            let len = row.content_len().max(required).min(row.cells.len());
            logical.reserve(len);
            let mut extras = std::mem::take(&mut row.extras).into_iter().peekable();
            for (col, cell) in row.cells.drain(..len).enumerate() {
                if cell.flags & Cell::LEADING_WIDE_SPACER != 0 {
                    continue;
                }
                while extras.next_if(|(extra_col, _)| *extra_col < col).is_some() {}
                if let Some((_, extra)) = extras.next_if(|(extra_col, _)| *extra_col == col) {
                    logical_extras.push((logical.len(), extra));
                }
                logical.push(cell);
            }
            output.recycle(row);
            if !joining {
                Self::wrap_logical(
                    &mut logical,
                    &mut logical_extras,
                    &mut output,
                    logical_cursor.take(),
                    logical_boundary.take(),
                    logical_viewport.take(),
                    cursor_pending,
                );
            }
        }
        if !logical.is_empty()
            || logical_cursor.is_some()
            || logical_boundary.is_some()
            || logical_viewport.is_some()
        {
            Self::wrap_logical(
                &mut logical,
                &mut logical_extras,
                &mut output,
                logical_cursor,
                logical_boundary,
                logical_viewport,
                cursor_pending,
            );
        }
        let cursor = output.cursor.expect("reflow maps the live cursor");
        // Rows below the last one with text were dropped above; they keep
        // their distance from the cursor's row.
        self.tracked_moves = Some(
            tracked
                .iter()
                .zip(&output.pins)
                .map(|(source, mapped)| {
                    let mapped = mapped.map_or_else(
                        || cursor.0 as i64 + (*source as i64 - cursor_source as i64),
                        |mapped| mapped as i64,
                    );
                    (*source, mapped - output.first as i64)
                })
                .collect(),
        );
        let mut split = output.produced.saturating_sub(size.rows).min(cursor.0);
        if self.clear_anchor {
            split = split.max(output.boundary.unwrap_or(0).min(cursor.0));
        }
        #[cfg(test)]
        {
            self.last_reflow_row_allocations = output.row_allocations;
        }
        self.primary.rows = output.rows.split_off(split - output.first);
        self.history = output.rows;
        self.recount_history_bytes();
        // Rows kept in place may still be packed; the live screen is dense.
        for row in &mut self.primary.rows {
            row.make_dense();
        }
        // Reflow leaves every history row dense. Repack them in the bounded
        // idle steps rather than all at once on the resize path.
        self.pending_compaction = self.history.len();
        self.primary.cursor.row = cursor.0 - split;
        self.primary.cursor.col = cursor.1;
        self.primary.pending_wrap = cursor.2;
        if let Some(viewport) = output.viewport {
            self.display_offset = split.saturating_sub(viewport.max(output.first));
        }
    }

    fn wrap_logical(
        cells: &mut Vec<Cell>,
        extras: &mut Vec<(usize, Arc<CellExtra>)>,
        output: &mut ReflowWindow,
        cursor: Option<usize>,
        boundary: Option<usize>,
        viewport: Option<usize>,
        cursor_pending: bool,
    ) {
        let cols = output.size.cols;
        let mut row = output.new_row();
        let mut col = 0;
        let mut cursor_mapped = false;
        let mut boundary_mapped = false;
        let mut viewport_mapped = false;
        let mut input = cells.drain(..).enumerate().peekable();
        let mut extras = extras.drain(..).peekable();
        let pins = std::mem::take(&mut output.logical_pins);
        let mut pins = pins.into_iter().peekable();
        while let Some((index, mut cell)) = input.next() {
            // Metadata of cells skipped below (narrowed spacers) is dropped.
            while extras
                .next_if(|(extra_index, _)| *extra_index < index)
                .is_some()
            {}
            let extra = extras
                .next_if(|(extra_index, _)| *extra_index == index)
                .map(|(_, extra)| extra);
            // A one-column screen must keep making progress, so wide glyphs
            // temporarily occupy one cell there. Recover their second cell
            // when the viewport grows again rather than permanently narrowing
            // the saved text.
            let expanded_wide = cols > 1
                && cell.flags & Cell::WIDE == 0
                && print::cell_width(&cell, extra.combining()) == 2;
            let wide = cell.flags & Cell::WIDE != 0 || expanded_wide;
            if col == cols || (wide && cols > 1 && col + 1 == cols) {
                if col < cols {
                    row.cells[col].flags = Cell::LEADING_WIDE_SPACER;
                }
                row.occupied = cols;
                row.wrapped = true;
                output.push(row);
                row = output.new_row();
                col = 0;
            }
            if cursor == Some(index) {
                output.cursor = Some((output.produced, col, false));
                cursor_mapped = true;
            }
            if boundary == Some(index) {
                output.boundary = Some(output.produced);
                boundary_mapped = true;
            }
            if viewport == Some(index) {
                output.viewport = Some(output.produced);
                viewport_mapped = true;
            }
            while let Some((_, pin)) = pins.next_if(|(cell, _)| *cell <= index) {
                output.pins[pin] = Some(output.produced);
            }
            if wide && cols == 1 {
                cell.flags &= !Cell::WIDE;
                if input
                    .peek()
                    .is_some_and(|(_, spacer)| spacer.flags & Cell::WIDE_SPACER != 0)
                {
                    let spacer_index = input.next().expect("peeked wide spacer").0;
                    if cursor == Some(spacer_index) {
                        output.cursor = Some((output.produced, col, false));
                        cursor_mapped = true;
                    }
                    if boundary == Some(spacer_index) {
                        output.boundary = Some(output.produced);
                        boundary_mapped = true;
                    }
                    if viewport == Some(spacer_index) {
                        output.viewport = Some(output.produced);
                        viewport_mapped = true;
                    }
                }
            }
            if expanded_wide {
                cell.flags |= Cell::WIDE;
                row.put(col + 1, cell.wide_spacer(), extra.spacer_extra(&mut None));
            }
            row.put(col, cell, extra);
            col += if expanded_wide { 2 } else { 1 };
            row.occupied = col;
        }
        if !cursor_mapped && cursor.is_some() {
            if col == cols && cursor_pending {
                output.cursor = Some((output.produced, cols - 1, true));
            } else if col == cols {
                row.wrapped = true;
                output.push(row);
                row = output.new_row();
                output.cursor = Some((output.produced, 0, false));
            } else {
                output.cursor = Some((output.produced, col, false));
            }
        }
        if !boundary_mapped && boundary.is_some() {
            output.boundary = Some(output.produced);
        }
        if !viewport_mapped && viewport.is_some() {
            output.viewport = Some(output.produced);
        }
        // A tracked row with no text of its own is the row that ends the line.
        for (_, pin) in pins {
            output.pins[pin] = Some(output.produced);
        }
        output.push(row);
    }
}

#[cfg(test)]
mod tests;
