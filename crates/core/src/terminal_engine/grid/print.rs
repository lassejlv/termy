//! Streaming graphemes and the ASCII output fast path.
use super::super::types::MAX_COMBINING_BYTES;
use super::{Cell, Grid};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn cell_width(cell: &Cell) -> usize {
    if cell.combining().is_empty() {
        return cell.character.width().unwrap_or(0);
    }
    let mut bytes = [0; MAX_COMBINING_BYTES + 4];
    let len = cell.character.encode_utf8(&mut bytes).len();
    bytes[len..len + cell.combining().len()].copy_from_slice(cell.combining().as_bytes());
    std::str::from_utf8(&bytes[..len + cell.combining().len()])
        .expect("cell UTF-8")
        .width()
}

// These base characters have a grapheme break between one another, even
// after a suffix without a trailing ZWJ. Hangul Jamo, Indic scripts, prepend
// marks, regional indicators, and emoji modifiers use the segmentation tables.
// Box drawing, braille, arrows, private-use (Powerline/Nerd Font) glyphs, CJK
// punctuation and fullwidth forms are common in TUIs, so they stay on this path.
fn ordinary_base(c: char) -> bool {
    // Branch on the block first so CJK and Hangul text, the common non-ASCII
    // case, tests only the ranges of its own plane.
    match c {
        '\u{3400}'..='\u{9fff}' | '\u{ac00}'..='\u{d7a3}' => true,
        '\u{0}'..='\u{2fff}' => {
            matches!(c, '\u{20}'..='\u{2ff}' | '\u{370}'..='\u{482}' | '\u{48a}'..='\u{52f}' | '\u{2010}'..='\u{2027}' | '\u{2030}'..='\u{205e}' | '\u{20a0}'..='\u{20cf}' | '\u{2100}'..='\u{2bff}')
        }
        '\u{3000}'..='\u{ffff}' => {
            matches!(c, '\u{3000}'..='\u{3029}' | '\u{3030}'..='\u{303f}' | '\u{3041}'..='\u{3096}' | '\u{309b}'..='\u{30ff}' | '\u{e000}'..='\u{f8ff}' | '\u{ff01}'..='\u{ff9d}' | '\u{ffa0}'..='\u{ffef}')
        }
        _ => matches!(c, '\u{1f300}'..='\u{1f3fa}' | '\u{1f400}'..='\u{1faff}'),
    }
}

/// These common character ranges have uniform scalar properties. Resolve both
/// properties together instead of looking up width and classifying again.
#[inline]
fn scalar_properties(character: char) -> (usize, bool) {
    match character {
        '\u{300}'..='\u{36f}' => (0, false),
        '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{ac00}'..='\u{d7a3}' => (2, true),
        '\u{c0}'..='\u{2ff}' | '\u{370}'..='\u{482}' | '\u{48a}'..='\u{52f}' => (1, true),
        _ => {
            let width = character.width().unwrap_or(0);
            (width, width != 0 && ordinary_base(character))
        }
    }
}

/// Extend, ZWJ and spacing marks join any preceding base, so checking them
/// against one ASCII letter avoids segmenting a long retained cluster.
#[cold]
#[inline(never)]
fn always_extends(c: char) -> bool {
    let mut bytes = [b'a'; 5];
    let len = 1 + c.encode_utf8(&mut bytes[1..]).len();
    std::str::from_utf8(&bytes[..len])
        .expect("grapheme UTF-8")
        .graphemes(true)
        .nth(1)
        .is_none()
}

impl Grid {
    fn extend_grapheme(&mut self, character: char) -> bool {
        if !self.grapheme_open {
            return false;
        }
        let row = self.cursor.row;
        let mut col = self.cursor.col;
        if !self.pending_wrap && !(self.grapheme_open && self.grapheme_at_margin) {
            let Some(previous) = col.checked_sub(1) else {
                return false;
            };
            col = previous;
        }
        if self.screen().rows[row].cells[col].flags & Cell::WIDE_SPACER != 0 {
            col = col.saturating_sub(1);
        }
        let previous = &self.screen().rows[row].cells[col];
        // Match the existing bound on untrusted combining sequences. Discard
        // excess suffixes without moving the cursor or splitting a cluster.
        let over_limit = previous.combining().len() + character.len_utf8() > MAX_COMBINING_BYTES;
        if over_limit && always_extends(character) {
            return true;
        }
        let mut bytes = [0; MAX_COMBINING_BYTES + 8];
        let mut len = previous.character.encode_utf8(&mut bytes).len();
        bytes[len..len + previous.combining().len()]
            .copy_from_slice(previous.combining().as_bytes());
        len += previous.combining().len();
        len += character.encode_utf8(&mut bytes[len..]).len();
        let text = std::str::from_utf8(&bytes[..len]).expect("grapheme UTF-8");
        if text.graphemes(true).nth(1).is_some() {
            return false;
        }
        if over_limit {
            return true;
        }
        let mut width = text.width().clamp(1, 2).min(self.size.cols);
        let old_width = 1 + usize::from(previous.flags & Cell::WIDE != 0);
        if !self.autowrap && col + width > self.size.cols {
            width = old_width;
        }
        let mut cell = previous.clone();
        self.combining_cache.append(&mut cell, character);
        if width == old_width {
            let active = &mut self.screen_mut().rows[row];
            active.cells[col] = cell;
            active.occupied = active.occupied.max(col + width);
            self.mark(row, col, col + width);
            return true;
        }
        let old_cursor = self.cursor;
        self.cursor.col = col;
        self.pending_wrap = false;
        let insert_mode = self.insert_mode;
        if insert_mode && width > old_width && col + width <= self.size.cols {
            self.cursor.col += old_width;
            self.insert_chars(width - old_width);
            self.cursor.col = col;
        }
        let blank = self.blank();
        let (cleared_start, cleared_end) =
            Self::clear_wide_at(&mut self.screen_mut().rows[row], col, &blank);
        // A narrower grapheme leaves an erased spacer outside the new write.
        // Mark it before put_cell can wrap or scroll, even with a hidden cursor.
        self.mark(row, cleared_start, cleared_end);
        if insert_mode && width < old_width {
            self.cursor.col = col + width;
            self.delete_chars(old_width - width);
            self.cursor.col = col;
        }
        self.insert_mode = false;
        self.put_cell(cell, width);
        self.insert_mode = insert_mode;
        self.mark_cursor(old_cursor);
        true
    }

    pub(in crate::terminal_engine) fn put_char(&mut self, character: char) {
        self.observe_output();
        let (width, ordinary) = scalar_properties(character);
        if (width != 0 || matches!(character, '\u{fe0e}' | '\u{fe0f}' | '\u{20e3}'))
            && !(self.grapheme_ordinary && ordinary)
            && self.extend_grapheme(character)
        {
            return;
        }
        if width == 0 {
            // A ZWJ can connect the next pictograph to this cell. Other
            // zero-width marks do not turn ordinary base pairs into a cluster.
            if character == '\u{200d}' {
                self.grapheme_ordinary = false;
            }
            let mut row = self.cursor.row;
            let mut col = self.cursor.col;
            if !self.pending_wrap && !(self.grapheme_open && self.grapheme_at_margin) {
                if self.grapheme_at_margin {
                    // A closed grapheme at the margin had its wide base
                    // dropped. Its marks must not attach to the cell before.
                    return;
                }
                if col > 0 {
                    col -= 1;
                } else if row > 0 && self.screen().rows[row - 1].wrapped {
                    row -= 1;
                    col = self.size.cols - 1;
                } else {
                    return;
                }
            }
            if self.screen().rows[row].cells[col].flags & Cell::WIDE_SPACER != 0 && col > 0 {
                col -= 1;
            }
            let active = if self.alternate_active {
                &mut self
                    .alternate
                    .as_mut()
                    .expect("active alternate screen")
                    .rows[row]
            } else {
                &mut self.primary.rows[row]
            };
            active.occupied = active.occupied.max(col + 1);
            let cell = &mut active.cells[col];
            self.combining_cache.append(cell, character);
            self.mark(row, col, col + 1);
            return;
        }

        let mut cell = self.pen.clone();
        cell.character = character;
        self.grapheme_ordinary = ordinary;
        self.put_cell(cell, width);
    }

    fn put_cell(&mut self, mut cell: Cell, width: usize) {
        if self.pending_wrap {
            if self.autowrap {
                let row = self.cursor.row;
                self.screen_mut().rows[row].wrapped = true;
                // Wrap metadata belongs to the last cell even when a hidden
                // cursor produces no damage at its previous position.
                self.mark(row, self.size.cols - 1, self.size.cols);
                self.cursor.col = 0;
                self.linefeed();
            }
            self.pending_wrap = false;
        }
        let width = width.min(self.size.cols).min(2);
        if width == 2 && self.cursor.col + 1 == self.size.cols {
            if !self.autowrap {
                // The glyph is discarded, so nothing may extend the cell to
                // its left as though it were this cluster.
                self.grapheme_open = false;
                self.grapheme_at_margin = true;
                return;
            }
            let row = self.cursor.row;
            let col = self.cursor.col;
            let blank = self.blank();
            let active = &mut self.screen_mut().rows[row];
            Self::clear_wide_at(active, col, &blank);
            active.cells[col].flags = Cell::LEADING_WIDE_SPACER;
            active.wrapped = true;
            self.mark(row, col.saturating_sub(1), col + 1);
            self.cursor.col = 0;
            self.linefeed();
        }
        if self.insert_mode {
            self.insert_chars(width);
        }

        let row = self.cursor.row;
        let col = self.cursor.col;
        let blank = self.blank();
        cell.flags = if width == 2 { Cell::WIDE } else { 0 };
        let active = &mut self.screen_mut().rows[row];
        let mut start = col;
        let mut end = col + width;
        // Only the outside halves of overwritten wide glyphs need erasing.
        // The destination cells are replaced below, so blanking them first
        // would write/drop each cell twice on ordinary Unicode output.
        if active.cells[col].flags & Cell::WIDE_SPACER != 0 && col > 0 {
            start -= 1;
            active.cells[start] = blank.clone();
        }
        if active.cells[end - 1].flags & Cell::WIDE != 0 && end < active.cells.len() {
            active.cells[end] = blank;
            end += 1;
        }
        if width == 2 {
            active.cells[col + 1] = Cell {
                character: ' ',
                style: cell.style,
                flags: Cell::WIDE_SPACER,
                extra: None,
            };
        }
        active.cells[col] = cell;
        active.occupied = active.occupied.max(end);
        self.grapheme_at_margin = col + width >= self.size.cols;
        if self.grapheme_at_margin {
            self.cursor.col = self.size.cols - 1;
            self.pending_wrap = self.autowrap;
        } else {
            self.cursor.col += width;
        }
        self.grapheme_open = true;
        if self.full_damage {
            // Graphics revisions must observe scalar edits even while text
            // consumers already need a full redraw and the cursor is hidden.
            self.mark(row, start, end);
            return;
        }
        if self.cursor.visible {
            // The written span covers the old cursor without wrapping; both
            // wrap paths mark its cell before moving or scrolling the row.
            end = end.max(self.cursor.col + 1);
        }
        self.mark(row, start, end);
    }

    /// Ordinary ASCII uses one bounds/damage update per row-local run.
    #[inline]
    pub(in crate::terminal_engine) fn write_ascii(&mut self, mut text: &[u8]) {
        debug_assert!(text.iter().all(|byte| (0x20..=0x7e).contains(byte)));
        // A prepend character can absorb the first ASCII scalar. Ordinary
        // ASCII runs still take the row-local fast path.
        if !self.grapheme_ordinary
            && let Some(&first) = text.first()
            && self.extend_grapheme(char::from(first))
        {
            text = &text[1..];
        }
        while !text.is_empty() {
            self.observe_output();
            if self.pending_wrap || self.insert_mode || !self.autowrap {
                self.put_char(char::from(text[0]));
                text = &text[1..];
                continue;
            }
            let row = self.cursor.row;
            let col = self.cursor.col;
            let count = text.len().min(self.size.cols - col);
            // Wide-cell repair is uncommon, and the scalar path handles both
            // ends of an overwritten glyph without a second general scan.
            if self.screen().rows[row].cells[col..col + count]
                .iter()
                .any(|cell| cell.flags != 0)
            {
                for &byte in &text[..count] {
                    self.put_char(char::from(byte));
                }
                text = &text[count..];
                continue;
            }
            let pen = self.pen.clone();
            let active = &mut self.screen_mut().rows[row];
            for (cell, &byte) in active.cells[col..col + count].iter_mut().zip(text) {
                cell.clone_from(&pen);
                cell.character = char::from(byte);
                cell.flags = 0;
            }
            active.occupied = active.occupied.max(col + count);
            self.cursor.col += count;
            if self.cursor.col == self.size.cols {
                self.cursor.col -= 1;
                self.pending_wrap = true;
            }
            self.grapheme_open = true;
            self.grapheme_ordinary = true;
            self.grapheme_at_margin = self.pending_wrap;
            self.mark(row, col, (col + count + 1).min(self.size.cols));
            text = &text[count..];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_properties_match_width_and_classification_for_every_unicode_scalar() {
        for character in (0..=0x10ffff).filter_map(char::from_u32) {
            let width = character.width().unwrap_or(0);
            assert_eq!(
                scalar_properties(character),
                (width, width != 0 && ordinary_base(character)),
                "U+{:04X}",
                character as u32
            );
        }
    }

    // The fast path skips segmentation between two ordinary bases, so every
    // such base must start a new cluster after another one.
    #[test]
    fn ordinary_bases_always_break_from_each_other() {
        for c in (0..=0x10ffff).filter_map(char::from_u32) {
            if !ordinary_base(c) || c.width().unwrap_or(0) == 0 {
                continue;
            }
            for text in [format!("a{c}"), format!("{c}a"), format!("{c}{c}")] {
                assert_eq!(text.graphemes(true).count(), 2, "U+{:04X}", c as u32);
            }
        }
    }

    #[test]
    fn always_extending_scalars_join_regardless_of_the_base() {
        for c in [
            '\u{301}',
            '\u{200d}',
            '\u{fe0f}',
            '\u{20e3}',
            '\u{1f3fd}',
            '\u{93f}',
        ] {
            assert!(always_extends(c), "U+{:04X}", c as u32);
        }
        for c in ['a', '💻', '🇩', '\u{1100}'] {
            assert!(!always_extends(c), "U+{:04X}", c as u32);
        }
    }
}
