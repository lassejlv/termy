//! Terminal cell storage and display flags.

use std::sync::Arc;

use bitflags::bitflags;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::Color;

bitflags! {
    #[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
    pub struct CellFlags: u16 {
        const BOLD = 1 << 0;
        const DIM = 1 << 1;
        const ITALIC = 1 << 2;
        const UNDERLINE = 1 << 3;
        const BLINK = 1 << 4;
        const INVERSE = 1 << 5;
        const HIDDEN = 1 << 6;
        const STRIKEOUT = 1 << 7;
        const DOUBLE_UNDERLINE = 1 << 8;
        const WIDE = 1 << 9;
        const WIDE_SPACER = 1 << 10;
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Cell {
    pub text: SmolStr,
    pub foreground: Color,
    pub background: Color,
    pub flags: CellFlags,
    pub hyperlink: Option<Arc<String>>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            text: SmolStr::new_inline(" "),
            foreground: Color::Default,
            background: Color::Default,
            flags: CellFlags::empty(),
            hyperlink: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct CellTemplate {
    pub foreground: Color,
    pub background: Color,
    pub flags: CellFlags,
    pub hyperlink: Option<Arc<String>>,
}

impl CellTemplate {
    pub(crate) fn blank(&self) -> Cell {
        Cell {
            text: SmolStr::new_inline(" "),
            foreground: self.foreground,
            background: self.background,
            flags: self.flags & !(CellFlags::WIDE | CellFlags::WIDE_SPACER),
            hyperlink: self.hyperlink.clone(),
        }
    }

    pub(crate) fn character(&self, character: char, width: usize) -> Cell {
        let mut encoded = [0; 4];
        let mut cell = self.blank();
        cell.text = SmolStr::new(character.encode_utf8(&mut encoded));
        if width == 2 {
            cell.flags.insert(CellFlags::WIDE);
        }
        cell
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::Cell;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn cell_storage_stays_compact() {
        assert_eq!(size_of::<Cell>(), 48);
    }
}
