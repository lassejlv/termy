use super::*;

impl HistoryRow {
    pub(super) fn stored_len(&self) -> usize {
        usize::from(self.stored)
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.cells.capacity()
    }

    fn encoded_capacity(&self) -> usize {
        self.cells.capacity()
    }
}

impl std::ops::Index<usize> for HistoryRow {
    type Output = Cell;

    fn index(&self, index: usize) -> &Self::Output {
        assert!(index < self.len(), "history cell index is in bounds");
        if index >= usize::from(self.stored) {
            &DEFAULT_CELL
        } else {
            panic!("compact history cells must be read through HistoryRow::get")
        }
    }
}

impl RowView<'_> {
    pub(crate) fn to_vec(self) -> Vec<Cell> {
        let mut cells = Vec::with_capacity(self.len());
        self.append_to(&mut cells, self.len());
        cells
    }
}

impl std::ops::Index<usize> for RowView<'_> {
    type Output = Cell;

    fn index(&self, index: usize) -> &Self::Output {
        match self {
            Self::Dense(cells) => &cells[index],
            Self::History(row) => &row[index],
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct HistoryStorageStats {
    pub(super) rows: usize,
    pub(super) logical_cells: usize,
    pub(super) encoded_capacity: usize,
    pub(super) retained_bytes: usize,
}

impl Grid {
    pub(super) fn history_storage_stats(&self) -> HistoryStorageStats {
        HistoryStorageStats {
            rows: self.history.len(),
            logical_cells: self.history.iter().map(HistoryRow::len).sum(),
            encoded_capacity: self.history.iter().map(HistoryRow::encoded_capacity).sum(),
            retained_bytes: self
                .history
                .capacity()
                .saturating_mul(std::mem::size_of::<HistoryRow>())
                .saturating_add(self.history.iter().map(HistoryRow::retained_bytes).sum()),
        }
    }
}
