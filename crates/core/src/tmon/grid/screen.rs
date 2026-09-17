use super::*;

#[derive(Clone, Debug)]
pub(super) struct Screen {
    pub(super) cols: usize,
    pub(super) rows: usize,
    pub(super) cells: Vec<Vec<Cell>>,
    pub(super) row_extents: Vec<usize>,
    pub(super) cursor_col: usize,
    pub(super) cursor_row: usize,
    pub(super) saved_cursor_col: usize,
    pub(super) saved_cursor_row: usize,
    pub(super) pen: Pen,
    pub(super) saved_pen: Pen,
    pub(super) charsets: [Charset; 4],
    pub(super) saved_charsets: [Charset; 4],
    pub(super) wrap_pending: bool,
    pub(super) saved_wrap_pending: bool,
    pub(super) scroll_top: usize,
    pub(super) scroll_bottom: usize,
    pub(super) plain_ascii_cells: bool,
}

impl Screen {
    pub(super) fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols,
            rows,
            cells: (0..rows).map(|_| vec![Cell::default(); cols]).collect(),
            row_extents: vec![0; rows],
            cursor_col: 0,
            cursor_row: 0,
            saved_cursor_col: 0,
            saved_cursor_row: 0,
            pen: Pen::default(),
            saved_pen: Pen::default(),
            charsets: [Charset::Ascii; 4],
            saved_charsets: [Charset::Ascii; 4],
            wrap_pending: false,
            saved_wrap_pending: false,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            plain_ascii_cells: true,
        }
    }

    pub(super) fn row(&self, row: usize) -> &[Cell] {
        &self.cells[row]
    }

    pub(super) fn row_mut(&mut self, row: usize) -> &mut [Cell] {
        self.row_extents[row] = self.cols;
        &mut self.cells[row]
    }

    pub(super) fn row_mut_through(&mut self, row: usize, end: usize) -> &mut [Cell] {
        self.row_extents[row] = self.row_extents[row].max(end.min(self.cols));
        &mut self.cells[row]
    }

    pub(super) fn row_extent(&self, row: usize) -> usize {
        self.row_extents[row]
    }

    pub(super) fn rebuild_row_extents(&mut self) {
        self.row_extents = self
            .cells
            .iter()
            .map(|row| HistoryRow::stored_len_for(row, self.cols))
            .collect();
    }

    pub(super) fn fill(&mut self, cell: Cell) {
        if cell == DEFAULT_CELL {
            for (row, extent) in self.cells.iter_mut().zip(&self.row_extents) {
                let clear_len = (*extent).min(row.len());
                row[..clear_len].fill(cell);
            }
        } else {
            for row in &mut self.cells {
                row.fill(cell);
            }
        }
        self.row_extents
            .fill(if cell == DEFAULT_CELL { 0 } else { self.cols });
        self.plain_ascii_cells = cell == DEFAULT_CELL;
    }

    pub(super) fn reset(&mut self) {
        self.fill(Cell::default());
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.saved_cursor_col = 0;
        self.saved_cursor_row = 0;
        self.pen = Pen::default();
        self.saved_pen = Pen::default();
        self.charsets = [Charset::Ascii; 4];
        self.saved_charsets = [Charset::Ascii; 4];
        self.wrap_pending = false;
        self.saved_wrap_pending = false;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
    }

    pub(super) fn save_cursor(&mut self) {
        self.saved_cursor_col = self.cursor_col;
        self.saved_cursor_row = self.cursor_row;
        self.saved_pen = self.pen;
        self.saved_charsets = self.charsets;
        self.saved_wrap_pending = self.wrap_pending;
    }

    pub(super) fn restore_cursor(&mut self) {
        self.cursor_col = self.saved_cursor_col.min(self.cols.saturating_sub(1));
        self.cursor_row = self.saved_cursor_row.min(self.rows.saturating_sub(1));
        self.pen = self.saved_pen;
        self.charsets = self.saved_charsets;
        self.wrap_pending = self.saved_wrap_pending;
    }

    pub(super) fn resize(
        &mut self,
        cols: usize,
        rows: usize,
        bottom_anchor: bool,
    ) -> Vec<Vec<Cell>> {
        let old_cols = self.cols;
        let old_rows = self.rows;
        let old_cursor_row = self.cursor_row;
        let mut removed = Vec::new();

        if bottom_anchor && rows < old_rows {
            for row in 0..old_rows - rows {
                removed.push(self.row(row).to_vec());
            }
        }

        let mut cells = (0..rows)
            .map(|_| vec![Cell::default(); cols])
            .collect::<Vec<_>>();
        let copy_cols = old_cols.min(cols);
        let copy_rows = old_rows.min(rows);
        let required_scrolling = if !bottom_anchor && rows < old_rows {
            old_cursor_row.saturating_add(1).saturating_sub(rows)
        } else {
            0
        };
        let (source_row, target_row) = if bottom_anchor {
            (
                old_rows.saturating_sub(copy_rows),
                rows.saturating_sub(copy_rows),
            )
        } else {
            (required_scrolling, 0)
        };

        for offset in 0..copy_rows {
            cells[target_row + offset][..copy_cols]
                .copy_from_slice(&self.cells[source_row + offset][..copy_cols]);
        }

        self.cols = cols;
        self.rows = rows;
        self.cells = cells;
        self.rebuild_row_extents();
        self.cursor_col = self.cursor_col.min(cols.saturating_sub(1));
        self.cursor_row = if bottom_anchor {
            if rows >= old_rows {
                old_cursor_row.saturating_add(rows - old_rows)
            } else {
                old_cursor_row.saturating_sub(old_rows - rows)
            }
        } else {
            old_cursor_row.min(rows.saturating_sub(1))
        }
        .min(rows.saturating_sub(1));
        self.saved_cursor_col = self.saved_cursor_col.min(cols.saturating_sub(1));
        self.saved_cursor_row = self.saved_cursor_row.min(rows.saturating_sub(1));
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
        self.plain_ascii_cells = false;
        removed
    }
}
