//! Damage accumulation for partial frame updates.

use serde::{Deserialize, Serialize};

use crate::{RowMove, RowMoveDirection};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct RowSpan {
    start: usize,
    end: usize,
}

impl Default for RowSpan {
    fn default() -> Self {
        Self {
            start: usize::MAX,
            end: 0,
        }
    }
}

impl RowSpan {
    const fn is_dirty(self) -> bool {
        self.start < self.end
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct Damage {
    full: bool,
    rows: Vec<RowSpan>,
    dirty_rows: Vec<usize>,
    snapshot_spans: Vec<DirtySpan>,
    row_moves: Vec<RowMove>,
    snapshot_moves: Vec<RowMove>,
    metadata: bool,
    revision: u64,
}

impl Damage {
    pub(crate) fn set_row_count(&mut self, row_count: usize) {
        self.dirty_rows.retain(|row| *row < row_count);
        self.rows.resize(row_count, RowSpan::default());
        let capacity_limit = reusable_capacity(row_count);
        if self.rows.capacity() > capacity_limit {
            let mut compact = Vec::with_capacity(row_count);
            compact.append(&mut self.rows);
            self.rows = compact;
        }
        if self.dirty_rows.capacity() > capacity_limit {
            let mut compact = Vec::with_capacity(self.dirty_rows.len());
            compact.append(&mut self.dirty_rows);
            self.dirty_rows = compact;
        }
        if self.snapshot_spans.capacity() > capacity_limit {
            self.snapshot_spans = Vec::with_capacity(row_count);
        }
        self.row_moves.clear();
        self.snapshot_moves.clear();
    }

    pub(crate) const fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn full(&mut self) {
        self.full = true;
        self.clear_pending_rows();
        self.row_moves.clear();
        self.metadata = true;
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn scroll(
        &mut self,
        start_row: usize,
        end_row: usize,
        direction: RowMoveDirection,
        count: usize,
    ) {
        let height = end_row.saturating_sub(start_row);
        let count = count.min(height);
        if self.full || count == 0 {
            self.revision = self.revision.wrapping_add(1);
            return;
        }
        if count == height {
            self.rows(start_row, end_row - 1);
            return;
        }

        match direction {
            RowMoveDirection::Up => {
                self.rows[start_row..end_row].rotate_left(count);
                for span in &mut self.rows[end_row - count..end_row] {
                    *span = RowSpan::default();
                }
                for row in &mut self.dirty_rows {
                    if (*row >= start_row) && (*row < end_row) {
                        *row = if *row < start_row + count {
                            usize::MAX
                        } else {
                            *row - count
                        };
                    }
                }
            }
            RowMoveDirection::Down => {
                self.rows[start_row..end_row].rotate_right(count);
                for span in &mut self.rows[start_row..start_row + count] {
                    *span = RowSpan::default();
                }
                for row in &mut self.dirty_rows {
                    if (*row >= start_row) && (*row < end_row) {
                        *row = if *row >= end_row - count {
                            usize::MAX
                        } else {
                            *row + count
                        };
                    }
                }
            }
        }
        self.dirty_rows.retain(|row| *row != usize::MAX);

        let exposed = match direction {
            RowMoveDirection::Up => (end_row - count)..end_row,
            RowMoveDirection::Down => start_row..(start_row + count),
        };
        for row in exposed {
            if !self.rows[row].is_dirty() {
                self.dirty_rows.push(row);
            }
            self.rows[row] = RowSpan {
                start: 0,
                end: usize::MAX,
            };
        }

        let movement = RowMove {
            start_row,
            end_row,
            direction,
            count,
        };
        if let Some(previous) = self.row_moves.last_mut()
            && previous.start_row == start_row
            && previous.end_row == end_row
            && previous.direction == direction
        {
            previous.count = previous.count.saturating_add(count);
            if previous.count >= height {
                self.full();
                return;
            }
        } else {
            self.row_moves.push(movement);
        }
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn span(&mut self, row: usize, start: usize, end: usize) {
        if !self.full && start < end {
            if self.rows.len() <= row {
                self.rows.resize(row + 1, RowSpan::default());
            }
            let span = &mut self.rows[row];
            if !span.is_dirty() {
                self.dirty_rows.push(row);
            }
            span.start = span.start.min(start);
            span.end = span.end.max(end);
        }
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn rows(&mut self, start: usize, end_inclusive: usize) {
        if self.full {
            self.revision = self.revision.wrapping_add(1);
            return;
        }
        if start <= end_inclusive {
            if self.rows.len() <= end_inclusive {
                self.rows.resize(end_inclusive + 1, RowSpan::default());
            }
            for row in start..=end_inclusive {
                if !self.rows[row].is_dirty() {
                    self.dirty_rows.push(row);
                }
                self.rows[row] = RowSpan {
                    start: 0,
                    end: usize::MAX,
                };
            }
        }
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn metadata(&mut self) {
        self.metadata = true;
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn take(
        &mut self,
        force_full: bool,
        columns: usize,
        row_count: usize,
    ) -> DamageSnapshot {
        let full = force_full || self.full;
        let mut spans = std::mem::take(&mut self.snapshot_spans);
        spans.clear();
        let mut moves = std::mem::take(&mut self.snapshot_moves);
        moves.clear();
        if full {
            spans.reserve(row_count);
            spans.extend((0..row_count).map(|row| DirtySpan {
                row,
                start: 0,
                end: columns,
            }));
            self.clear_pending_rows();
            self.row_moves.clear();
        } else {
            moves.append(&mut self.row_moves);
            self.dirty_rows.sort_unstable();
            for row in self.dirty_rows.drain(..) {
                let span = std::mem::take(&mut self.rows[row]);
                if row < row_count {
                    let start = span.start.min(columns);
                    let end = span.end.min(columns);
                    if start < end {
                        spans.push(DirtySpan { row, start, end });
                    }
                }
            }
        }
        let metadata = force_full || self.metadata;
        self.full = false;
        self.metadata = false;
        DamageSnapshot {
            full,
            spans,
            moves,
            metadata,
            revision: self.revision,
        }
    }

    pub(crate) fn recycle_snapshot(&mut self, mut spans: Vec<DirtySpan>, mut moves: Vec<RowMove>) {
        spans.clear();
        self.snapshot_spans = spans;
        moves.clear();
        self.snapshot_moves = moves;
    }

    pub(crate) fn capacities(&self) -> (usize, usize) {
        (
            self.rows
                .capacity()
                .saturating_add(self.dirty_rows.capacity()),
            self.snapshot_spans.capacity(),
        )
    }

    fn clear_pending_rows(&mut self) {
        for row in self.dirty_rows.drain(..) {
            self.rows[row] = RowSpan::default();
        }
    }
}

fn reusable_capacity(length: usize) -> usize {
    length.saturating_mul(4).max(length.saturating_add(64))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct DirtySpan {
    pub row: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug)]
pub(crate) struct DamageSnapshot {
    pub full: bool,
    pub spans: Vec<DirtySpan>,
    pub moves: Vec<RowMove>,
    pub metadata: bool,
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use crate::RowMoveDirection;

    use super::{Damage, DirtySpan};

    #[test]
    fn row_indexed_damage_is_sorted_and_coalesced() {
        let mut damage = Damage::default();
        damage.span(4, 3, 7);
        damage.span(1, 2, 4);
        damage.span(4, 1, 5);
        damage.rows(2, 3);

        let snapshot = damage.take(false, 6, 5);
        assert_eq!(
            snapshot.spans,
            vec![
                DirtySpan {
                    row: 1,
                    start: 2,
                    end: 4,
                },
                DirtySpan {
                    row: 2,
                    start: 0,
                    end: 6,
                },
                DirtySpan {
                    row: 3,
                    start: 0,
                    end: 6,
                },
                DirtySpan {
                    row: 4,
                    start: 1,
                    end: 6,
                },
            ]
        );
    }

    #[test]
    fn snapshot_span_allocation_is_reused_between_frames() {
        let mut damage = Damage::default();
        damage.rows(0, 7);
        let first = damage.take(false, 80, 8);
        let allocation = first.spans.as_ptr();
        let capacity = first.spans.capacity();
        assert!(capacity >= 8);
        damage.recycle_snapshot(first.spans, first.moves);

        damage.span(3, 1, 2);
        let second = damage.take(false, 80, 8);
        assert_eq!(second.spans.as_ptr(), allocation);
        assert_eq!(second.spans.capacity(), capacity);
        assert_eq!(
            second.spans,
            vec![DirtySpan {
                row: 3,
                start: 1,
                end: 2,
            }]
        );
    }

    #[test]
    fn full_damage_reuses_row_and_snapshot_capacity() {
        let mut damage = Damage::default();
        damage.span(31, 0, 1);
        let row_capacity = damage.rows.capacity();
        damage.full();
        let snapshot = damage.take(false, 4, 3);
        assert!(snapshot.full);
        assert_eq!(snapshot.spans.len(), 3);
        damage.recycle_snapshot(snapshot.spans, snapshot.moves);

        assert_eq!(damage.rows.capacity(), row_capacity);
        assert!(damage.snapshot_spans.capacity() >= 3);
    }

    #[test]
    fn shrinking_a_very_tall_grid_releases_damage_capacity() {
        let mut damage = Damage::default();
        damage.set_row_count(4096);
        damage.full();
        let snapshot = damage.take(false, 80, 4096);
        damage.recycle_snapshot(snapshot.spans, snapshot.moves);

        damage.set_row_count(8);
        let (rows, snapshots) = damage.capacities();
        assert!(rows <= super::reusable_capacity(8));
        assert!(snapshots <= super::reusable_capacity(8));
    }

    #[test]
    fn scroll_moves_existing_damage_and_marks_only_exposed_rows() {
        let mut damage = Damage::default();
        damage.set_row_count(5);
        let initial = damage.take(false, 8, 5);
        damage.recycle_snapshot(initial.spans, initial.moves);
        damage.span(2, 3, 4);

        damage.scroll(0, 5, RowMoveDirection::Up, 1);
        let snapshot = damage.take(false, 8, 5);

        assert_eq!(snapshot.moves.len(), 1);
        assert_eq!(snapshot.moves[0].count, 1);
        assert_eq!(snapshot.spans.len(), 2);
        assert_eq!(
            snapshot.spans[0],
            DirtySpan {
                row: 1,
                start: 3,
                end: 4
            }
        );
        assert_eq!(
            snapshot.spans[1],
            DirtySpan {
                row: 4,
                start: 0,
                end: 8
            }
        );
    }

    #[test]
    fn consecutive_compatible_scrolls_coalesce() {
        let mut damage = Damage::default();
        damage.set_row_count(6);
        let initial = damage.take(false, 8, 6);
        damage.recycle_snapshot(initial.spans, initial.moves);

        damage.scroll(1, 5, RowMoveDirection::Down, 1);
        damage.scroll(1, 5, RowMoveDirection::Down, 1);
        let snapshot = damage.take(false, 8, 6);

        assert_eq!(snapshot.moves.len(), 1);
        assert_eq!(snapshot.moves[0].count, 2);
        assert_eq!(
            snapshot
                .spans
                .iter()
                .map(|span| span.row)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
}
