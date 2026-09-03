//! Full and partial frame-update contracts.

use crate::{Cell, SelectionRange};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CursorState {
    pub row: usize,
    pub column: usize,
    pub visible: bool,
    pub shape: CursorShape,
    pub blinking: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RowUpdate {
    pub index: usize,
    pub start_column: usize,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RowMoveDirection {
    Up,
    Down,
}

/// Moves retained rows inside `[start_row, end_row)` before row updates are patched.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RowMove {
    pub start_row: usize,
    pub end_row: usize,
    pub direction: RowMoveDirection,
    pub count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrameUpdate {
    pub full: bool,
    pub columns: usize,
    pub rows: usize,
    pub row_moves: Vec<RowMove>,
    pub row_updates: Vec<RowUpdate>,
    pub metadata_changed: bool,
    pub cursor: CursorState,
    pub selection: Option<SelectionRange>,
    pub display_offset: usize,
    pub revision: u64,
}

impl FrameUpdate {
    #[must_use]
    pub fn has_damage(&self) -> bool {
        self.full
            || self.metadata_changed
            || !self.row_moves.is_empty()
            || !self.row_updates.is_empty()
    }
}
