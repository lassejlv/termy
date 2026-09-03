//! Grid selection coordinates and normalized ranges.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SelectionPoint {
    pub column: usize,
    pub row: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SelectionRange {
    pub start: SelectionPoint,
    pub end: SelectionPoint,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum SelectionMode {
    #[default]
    Character,
    Word,
    Line,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Selection {
    anchor: SelectionRange,
    head: SelectionRange,
    mode: SelectionMode,
}

impl Selection {
    pub(crate) const fn new(anchor: SelectionRange, mode: SelectionMode) -> Self {
        Self {
            anchor,
            head: anchor,
            mode,
        }
    }

    pub(crate) fn set_head(&mut self, head: SelectionRange) -> bool {
        if self.head == head {
            return false;
        }
        self.head = head;
        true
    }

    pub(crate) const fn mode(self) -> SelectionMode {
        self.mode
    }

    pub(crate) fn range(self) -> SelectionRange {
        let start = if point_key(self.anchor.start) <= point_key(self.head.start) {
            self.anchor.start
        } else {
            self.head.start
        };
        let end = if point_key(self.anchor.end) >= point_key(self.head.end) {
            self.anchor.end
        } else {
            self.head.end
        };
        SelectionRange { start, end }
    }
}

const fn point_key(point: SelectionPoint) -> (usize, usize) {
    (point.row, point.column)
}

impl SelectionRange {
    pub(crate) const fn point(point: SelectionPoint) -> Self {
        Self {
            start: point,
            end: point,
        }
    }
}

pub(crate) fn clamp_point(point: SelectionPoint, columns: usize, rows: usize) -> SelectionPoint {
    SelectionPoint {
        column: point.column.min(columns.saturating_sub(1)),
        row: point.row.min(rows.saturating_sub(1)),
    }
}
