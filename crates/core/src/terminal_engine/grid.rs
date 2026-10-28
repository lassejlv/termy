//! Row-oriented terminal storage. Scrolling moves row handles, not cells.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use super::types::{
    Cell, CellExtra, Color, Cursor, CursorShape, Damage, DirtySpan, GridEffect, Line, Pen, Size,
    Style, ViewportScroll,
};

mod cjk;
mod combining;
mod damage;
mod extras;
mod flood;
mod history;
mod margins;
mod narrow;
mod print;
mod reflow;
mod ring;
mod row;
mod snapshot;
use combining::CombiningCache;
use flood::PendingClear;
pub(super) use history::DEFAULT_HISTORY_BYTES;
use ring::Ring;
use row::PackedCells;

const MAX_SCROLL_DAMAGE: usize = 32;
const COMPACT_OUTPUT_BURST_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub(super) struct Row {
    cells: Vec<Cell>,
    // Metadata of the cells that have some, by column; see `extras.rs`.
    extras: Vec<(usize, Arc<CellExtra>)>,
    packed: Option<Box<PackedCells>>,
    // Cells after this prefix still equal the blank used by the last clear.
    // Keeping a conservative upper bound avoids rewriting an entire row when
    // short output lines recycle it through scrollback.
    occupied: usize,
    clear_background: Color,
    pub(super) wrapped: bool,
    // Metadata bytes measured as the row entered history; see `row.rs`.
    extra_units: u16,
}

impl Row {
    // The suffix tracked below contains erase blanks: only their background
    // varies. Text, attributes, wide flags and metadata belong in `occupied`.
    fn new(cols: usize, blank: &Cell) -> Self {
        Self {
            cells: vec![*blank; cols],
            extras: Vec::new(),
            packed: None,
            occupied: 0,
            clear_background: blank.style.background,
            wrapped: false,
            extra_units: 0,
        }
    }

    fn clear(&mut self, cols: usize, blank: &Cell) {
        let end = self.clear_deferring_fill(cols, blank);
        Cell::fill_plain(&mut self.cells[..end], blank.character, blank.style);
    }

    /// Clear the row except blanking its first cells: returns how many
    /// leading cells still hold old content and must be filled with `blank`
    /// before anything reads them.
    fn clear_deferring_fill(&mut self, cols: usize, blank: &Cell) -> usize {
        // Recycled history rows are usually packed. Their contents are about
        // to be blanked, so reuse a decoded cache if any but skip decoding.
        if self.discard_packed() {
            self.occupied = 0;
        }
        let end = if self.clear_background == blank.style.background {
            self.occupied.min(cols)
        } else {
            cols
        };
        let len = self.cells.len().min(cols);
        if self.cells.len() != cols {
            self.cells.truncate(cols);
            self.cells.resize(cols, *blank);
        }
        self.extras.clear();
        self.occupied = 0;
        self.clear_background = blank.style.background;
        self.wrapped = false;
        self.extra_units = 0;
        end.min(len)
    }

    fn content_len(&self) -> usize {
        let cells = self.cells();
        if self.wrapped {
            return cells.len();
        }
        let text = cells
            .iter()
            .rposition(|cell| cell != &Cell::default())
            .map_or(0, |index| index + 1);
        // A default cell with metadata (a linked or underlined space) counts.
        self.extras
            .last()
            .map_or(text, |(col, _)| text.max(col + 1).min(cells.len()))
    }
}

#[derive(Clone, Default)]
struct SavedCursor {
    // Only the position is restored. Visibility, shape and blinking are
    // terminal-wide, as in xterm, so DECRC never changes them.
    cursor: Cursor,
    pen: Pen,
    pending_wrap: bool,
    origin_mode: bool,
    autowrap: bool,
    // DECRQM ?1048 reports whether this screen holds a saved cursor.
    valid: bool,
}

struct Screen {
    rows: Ring<Row>,
    saved: SavedCursor,
    cursor: Cursor,
    pending_wrap: bool,
}

impl Screen {
    fn new(size: Size) -> Self {
        Self {
            rows: (0..size.rows)
                .map(|_| Row::new(size.cols, &Cell::default()))
                .collect(),
            saved: SavedCursor {
                autowrap: true,
                ..SavedCursor::default()
            },
            cursor: Cursor::default(),
            pending_wrap: false,
        }
    }
}

pub(super) struct Grid {
    size: Size,
    primary: Screen,
    alternate: Option<Screen>,
    alternate_active: bool,
    history: Ring<Row>,
    // Bytes charged to `history` rows; see `history.rs`.
    history_bytes: usize,
    history_limit: usize,
    history_byte_limit: usize,
    pending_compaction: usize,
    compact_on_scroll: bool,
    output_since_compaction: usize,
    display_offset: usize,
    // Spans the widest width seen, so stops survive a shrink and regrow.
    tabs: Vec<bool>,
    scroll_top: usize,
    scroll_bottom: usize,
    // DECSLRM; `0..cols` unless DECLRMM is set. See `margins.rs`.
    left_margin: usize,
    right_margin: usize,
    lr_margin_mode: bool,
    pending_wrap: bool,
    // Erasing the display establishes a boundary that width reflow must not
    // cross when choosing the new live viewport.
    clear_anchor: bool,
    full_damage: bool,
    // Marking cells can change no damage: full damage is pending, or a
    // pending scroll exposed the whole screen, so every row is fully dirty.
    damage_saturated: bool,
    // Damage is saturated and the last pending scroll moved the whole screen
    // up by its height: a live flood's scroll changes no damage. Only ever
    // set while that holds; see `mark_live_scroll_damage`.
    flood_exposed: bool,
    visual_dirty: bool,
    dirty: Vec<Option<(usize, usize)>>,
    pending_scrolls: Vec<ViewportScroll>,
    combining_cache: CombiningCache,
    // Last color-only spacer extra; see `Cell::spacer_extra`.
    spacer_extra: Option<Arc<CellExtra>>,
    // Cells of the row a line feed just scrolled in that still hold old
    // content. The line's text runs cover them from the left; whatever they
    // skip or leave is filled before anything else reads the row (see
    // `flush_pending_clear`).
    pending_clear: PendingClear,
    defer_scroll_clear: bool,
    grapheme_open: bool,
    grapheme_ordinary: bool,
    // Only meaningful while a grapheme is open. A closed grapheme at the
    // margin marks a wide base dropped there with autowrap disabled.
    grapheme_at_margin: bool,
    pub(super) history_activity: bool,
    // Rows output has scrolled into history, for content-anchored ranges.
    // Resize and reflow move rows without counting them.
    scrolled_into_history: u64,
    // Borrowed history reads usually decode one contiguous viewport. Track
    // that interval so the next mutation does not scan untouched history.
    history_read_start: AtomicUsize,
    history_read_end: AtomicUsize,
    track_effects: bool,
    effects: Vec<GridEffect>,
    #[cfg(test)]
    last_reflow_row_allocations: usize,
    pub(super) cursor: Cursor,
    pub(super) pen: Pen,
    pub(super) autowrap: bool,
    pub(super) origin_mode: bool,
    pub(super) insert_mode: bool,
    pub(super) newline_mode: bool,
}

impl Grid {
    pub(super) fn prepare_output(&mut self, bytes: usize) {
        // PTY reads can fragment a sustained burst into small pieces. Count
        // the whole burst so on-scroll packing stops regardless of chunking.
        if self.compact_on_scroll {
            self.output_since_compaction = self.output_since_compaction.saturating_add(bytes);
            if self.output_since_compaction >= COMPACT_OUTPUT_BURST_BYTES {
                self.compact_on_scroll = false;
            }
        }
        self.release_history_read_cache();
    }

    pub(super) fn needs_compaction(&self) -> bool {
        self.pending_compaction.min(self.history.len()) != 0
    }

    pub(super) fn compact_history(&mut self, limit: usize) {
        self.compact_pending_history(limit);
        self.compact_on_scroll = true;
        self.output_since_compaction = 0;
    }

    #[inline]
    pub(super) fn release_history_read_cache(&mut self) {
        if *self.history_read_end.get_mut() == 0 {
            return;
        }
        self.release_history_read_cache_slow();
    }

    #[inline(never)]
    fn release_history_read_cache_slow(&mut self) {
        let start = std::mem::replace(self.history_read_start.get_mut(), usize::MAX);
        // Every history mutation releases first, so `end` should still be in
        // bounds; clamping keeps a missed caller from panicking here.
        let end = std::mem::take(self.history_read_end.get_mut()).min(self.history.len());
        if start < end {
            for row in self.history.range_mut(start..end) {
                row.release_read_cache();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn has_history_read_cache(&self) -> bool {
        self.history_read_end.load(Ordering::Relaxed) != 0
    }

    #[cfg(test)]
    pub(super) fn new(size: Size, history_limit: usize) -> Self {
        Self::with_limits(size, history_limit, history::DEFAULT_HISTORY_BYTES)
    }

    /// History keeps at most `history_limit` rows and `history_byte_limit`
    /// charged bytes, whichever binds first; `usize::MAX` rows leaves only the
    /// byte limit.
    pub(super) fn with_limits(size: Size, history_limit: usize, history_byte_limit: usize) -> Self {
        let size = size.clamped();
        Self {
            size,
            primary: Screen::new(size),
            alternate: None,
            alternate_active: false,
            history: Ring::new(),
            history_bytes: 0,
            history_limit,
            history_byte_limit,
            pending_compaction: 0,
            compact_on_scroll: false,
            output_since_compaction: 0,
            display_offset: 0,
            tabs: Self::default_tabs(size.cols),
            scroll_top: 0,
            scroll_bottom: size.rows,
            left_margin: 0,
            right_margin: size.cols,
            lr_margin_mode: false,
            pending_wrap: false,
            clear_anchor: false,
            full_damage: true,
            damage_saturated: true,
            flood_exposed: false,
            visual_dirty: false,
            dirty: vec![None; size.rows],
            pending_scrolls: Vec::with_capacity(MAX_SCROLL_DAMAGE),
            combining_cache: CombiningCache::default(),
            spacer_extra: None,
            pending_clear: PendingClear::NONE,
            defer_scroll_clear: false,
            grapheme_open: false,
            grapheme_ordinary: true,
            grapheme_at_margin: false,
            history_activity: false,
            scrolled_into_history: 0,
            history_read_start: AtomicUsize::new(usize::MAX),
            history_read_end: AtomicUsize::new(0),
            track_effects: false,
            effects: Vec::new(),
            #[cfg(test)]
            last_reflow_row_allocations: 0,
            cursor: Cursor::default(),
            pen: Pen::default(),
            autowrap: true,
            origin_mode: false,
            insert_mode: false,
            newline_mode: false,
        }
    }

    fn default_tabs(cols: usize) -> Vec<bool> {
        (0..cols).map(|col| col != 0 && col % 8 == 0).collect()
    }
    fn screen(&self) -> &Screen {
        if self.alternate_active {
            self.alternate.as_ref().expect("active alternate screen")
        } else {
            &self.primary
        }
    }
    fn screen_mut(&mut self) -> &mut Screen {
        if self.alternate_active {
            self.alternate.as_mut().expect("active alternate screen")
        } else {
            &mut self.primary
        }
    }
    pub(super) fn size(&self) -> Size {
        self.size
    }
    pub(super) fn history_size(&self) -> usize {
        if self.alternate_active {
            0
        } else {
            self.history.len()
        }
    }
    pub(super) fn display_offset(&self) -> usize {
        if self.alternate_active {
            0
        } else {
            self.display_offset
        }
    }
    pub(super) fn alternate_screen(&self) -> bool {
        self.alternate_active
    }
    pub(super) fn scrolled_into_history(&self) -> u64 {
        self.scrolled_into_history
    }
    pub(super) fn history_limit(&self) -> usize {
        self.history_limit
    }
    pub(super) fn history_byte_limit(&self) -> usize {
        self.history_byte_limit
    }
    pub(super) fn scroll_region(&self) -> (usize, usize) {
        (self.scroll_top, self.scroll_bottom)
    }

    /// History has negative line numbers; the live screen starts at zero.
    pub(super) fn row(&self, line: i32) -> Option<&Row> {
        if line >= 0 {
            return self.screen().rows.get(line as usize);
        }
        if self.alternate_active {
            return None;
        }
        self.history
            .len()
            .checked_sub(line.unsigned_abs() as usize)
            .and_then(|index| self.history.get(index))
    }

    pub(super) fn visible_row(&self, row: usize) -> Option<&Row> {
        if row >= self.size.rows {
            return None;
        }
        self.row(row as i32 - self.display_offset() as i32)
    }

    pub(super) fn row_line(&self, line: i32) -> Option<Line<'_>> {
        let row = self.row(line)?;
        if line < 0 && row.packed.is_some() {
            let index = self.history.len() - line.unsigned_abs() as usize;
            self.history_read_start.fetch_min(index, Ordering::Relaxed);
            self.history_read_end
                .fetch_max(index + 1, Ordering::Relaxed);
        }
        Some(row.line())
    }

    pub(super) fn visible_row_line(&self, row: usize) -> Option<Line<'_>> {
        if row >= self.size.rows {
            return None;
        }
        self.row_line(row as i32 - self.display_offset() as i32)
    }

    pub(super) fn tracks_effects(&self) -> bool {
        self.track_effects
    }

    pub(super) fn set_effect_tracking(&mut self, enabled: bool) {
        self.track_effects = enabled;
        if !enabled {
            self.effects.clear();
        }
    }

    pub(super) fn drain_effects(&mut self, output: &mut Vec<GridEffect>) {
        output.append(&mut self.effects);
    }

    fn effect(&mut self, effect: GridEffect) {
        if !self.track_effects {
            return;
        }
        if let GridEffect::Scroll {
            alternate,
            top,
            bottom,
            lines,
            retains_history,
            history_before,
            history_after,
        } = effect
            && let Some(GridEffect::Scroll {
                alternate: previous_alternate,
                top: previous_top,
                bottom: previous_bottom,
                lines: previous_lines,
                retains_history: previous_retains_history,
                history_after: previous_history_after,
                ..
            }) = self.effects.last_mut()
            && *previous_alternate == alternate
            && *previous_top == top
            && *previous_bottom == bottom
            && previous_lines.signum() == lines.signum()
            && *previous_retains_history == retains_history
            && *previous_history_after == history_before
        {
            *previous_lines = previous_lines.saturating_add(lines);
            *previous_history_after = history_after;
            return;
        }
        self.effects.push(effect);
    }

    pub(super) fn end_grapheme(&mut self) {
        self.grapheme_open = false;
        self.grapheme_at_margin = false;
    }

    #[inline]
    fn motion_done(&mut self, old: Cursor) {
        self.end_grapheme();
        self.pending_wrap = false;
        if !self.damage_saturated {
            self.mark_cursor(old);
            self.mark_cursor(self.cursor);
        }
    }

    pub(super) fn cursor_changed(&mut self, old: Cursor) {
        self.mark_cursor(old);
        self.mark_cursor(self.cursor);
    }

    fn blank(&self) -> Cell {
        Self::blank_on(self.pen.style.background)
    }

    fn blank_on(background: Color) -> Cell {
        Cell {
            style: Style {
                background,
                ..Style::default()
            },
            ..Cell::default()
        }
    }

    fn clear_wide_at(row: &mut Row, col: usize, blank: &Cell) -> (usize, usize) {
        let mut start = col;
        let mut end = col + 1;
        if row.cells[col].flags & Cell::WIDE_SPACER != 0 && col > 0 {
            start -= 1;
            row.blank_cell(start, *blank);
        }
        if row.cells[col].flags & Cell::WIDE != 0 && end < row.cells.len() {
            row.blank_cell(end, *blank);
            end += 1;
        }
        row.blank_cell(col, *blank);
        row.occupied = row.occupied.max(end);
        (start, end)
    }

    pub(super) fn carriage_return(&mut self) {
        let old = self.cursor;
        self.cursor.col = self.left_limit();
        self.motion_done(old);
    }
    pub(super) fn backspace(&mut self) {
        let old = self.cursor;
        if self.cursor.col != self.left_margin {
            self.cursor.col = self.cursor.col.saturating_sub(1);
        }
        self.motion_done(old);
    }

    pub(super) fn linefeed(&mut self) {
        let old = self.cursor;
        if self.cursor.row + 1 == self.scroll_bottom {
            // Outside the left/right margins the bottom margin just stops.
            if self.in_lr_margins() {
                self.scroll_up(1);
            }
        } else {
            self.cursor.row = (self.cursor.row + 1).min(self.size.rows - 1);
        }
        if self.newline_mode {
            self.cursor.col = self.left_limit();
        }
        self.motion_done(old);
        self.observe_output();
    }

    fn observe_output(&mut self) {
        if !self.alternate_active && self.cursor.row + 1 == self.size.rows {
            self.clear_anchor = false;
        }
    }

    pub(super) fn reverse_index(&mut self) {
        let old = self.cursor;
        if self.cursor.row == self.scroll_top {
            if self.in_lr_margins() {
                self.scroll_down(1);
            }
        } else {
            self.cursor.row = self.cursor.row.saturating_sub(1);
        }
        self.motion_done(old);
    }

    /// CUP: in origin mode both coordinates are relative to the margins.
    pub(super) fn goto(&mut self, row: usize, col: usize) {
        let old = self.cursor;
        self.cursor.row = self.origin_row(row);
        self.cursor.col = self.origin_col(col);
        self.motion_done(old);
    }

    /// VPA: like CUP, but keeps the column.
    pub(super) fn goto_row(&mut self, row: usize) {
        let old = self.cursor;
        self.cursor.row = self.origin_row(row);
        self.motion_done(old);
    }

    /// CHA/HPA: like CUP, but keeps the row.
    pub(super) fn goto_col(&mut self, col: usize) {
        let old = self.cursor;
        self.cursor.col = self.origin_col(col);
        self.motion_done(old);
    }

    fn origin_row(&self, row: usize) -> usize {
        if self.origin_mode {
            row.saturating_add(self.scroll_top)
                .min(self.scroll_bottom - 1)
        } else {
            row.min(self.size.rows - 1)
        }
    }

    fn origin_col(&self, col: usize) -> usize {
        if self.origin_mode {
            col.saturating_add(self.left_margin)
                .min(self.right_margin - 1)
        } else {
            col.min(self.size.cols - 1)
        }
    }

    pub(super) fn move_cursor(&mut self, row_delta: isize, col_delta: isize) {
        let old = self.cursor;
        // Like xterm, CUU stops at the top margin unless the cursor starts
        // above it, and CUD stops at the bottom margin unless it starts below.
        let top = if self.origin_mode || self.cursor.row >= self.scroll_top {
            self.scroll_top
        } else {
            0
        };
        let bottom = if self.origin_mode || self.cursor.row < self.scroll_bottom {
            self.scroll_bottom
        } else {
            self.size.rows
        };
        self.cursor.row = self
            .cursor
            .row
            .saturating_add_signed(row_delta)
            .clamp(top, bottom - 1);
        // CUF and CUB treat the left/right margins the same way.
        let left = if self.origin_mode || self.cursor.col >= self.left_margin {
            self.left_margin
        } else {
            0
        };
        let right = if self.origin_mode || self.cursor.col < self.right_margin {
            self.right_margin
        } else {
            self.size.cols
        };
        self.cursor.col = self
            .cursor
            .col
            .saturating_add_signed(col_delta)
            .clamp(left, right - 1);
        self.motion_done(old);
    }

    pub(super) fn tab(&mut self) {
        let old = self.cursor;
        let right = self.right_limit();
        self.cursor.col = ((self.cursor.col + 1)..right)
            .find(|&col| self.tabs[col])
            .unwrap_or(right - 1);
        self.motion_done(old);
    }
    pub(super) fn backtab(&mut self) {
        let old = self.cursor;
        let left = self.left_limit();
        self.cursor.col = (left..self.cursor.col)
            .rev()
            .find(|&col| self.tabs[col])
            .unwrap_or(left);
        self.motion_done(old);
    }
    pub(super) fn set_tab(&mut self) {
        self.tabs[self.cursor.col] = true;
    }
    pub(super) fn clear_tab(&mut self, all: bool) {
        if all {
            self.tabs.fill(false);
        } else {
            self.tabs[self.cursor.col] = false;
        }
    }

    fn erase_range(&mut self, row: usize, start: usize, end: usize, selective: bool) {
        if start >= end {
            return;
        }
        let blank = self.blank();
        let active = &mut self.screen_mut().rows[row];
        if !selective {
            let occupied = active.occupied;
            let background = blank.style.background;
            let (first_left, first_right) = Self::clear_wide_at(active, start, &blank);
            let (last_left, last_right) = Self::clear_wide_at(active, end - 1, &blank);
            // Past `occupied`, cells already equal a blank on the same
            // background, so erasing to the end of a short line writes little.
            let filled = if active.clear_background == background {
                end.min(occupied.max(start))
            } else {
                end
            };
            Cell::fill_plain(&mut active.cells[start..filled], ' ', blank.style);
            active.clear_extras(start..end);
            if end == active.cells.len() {
                active.wrapped = false;
                // Every cell from the erase start is now this blank, so the
                // clean suffix starts there instead of spanning the row.
                active.occupied = if active.clear_background == background {
                    occupied.min(first_left)
                } else {
                    first_left
                };
                active.clear_background = background;
            }
            self.mark(row, first_left.min(last_left), first_right.max(last_right));
            return;
        }
        let mut changed_start = end;
        let mut changed_end = start;
        let mut col = start;
        while col < end {
            let protected = active.cells[col].style.attributes & Style::PROTECTED != 0;
            if !selective || !protected {
                let (left, right) = Self::clear_wide_at(active, col, &blank);
                changed_start = changed_start.min(left);
                changed_end = changed_end.max(right);
            }
            col += 1;
        }
        if end == active.cells.len() && (!selective || changed_end == end) {
            active.wrapped = false;
        }
        self.mark(row, changed_start, changed_end);
    }

    pub(super) fn erase_display(&mut self, mode: u16, selective: bool) {
        self.pending_wrap = false;
        let full_clear = mode == 2
            || (mode == 0 && self.cursor.row == 0 && self.cursor.col == 0)
            || (mode == 1
                && self.cursor.row + 1 == self.size.rows
                && self.cursor.col + 1 == self.size.cols);
        match mode {
            0 => {
                self.erase_range(self.cursor.row, self.cursor.col, self.size.cols, selective);
                for row in self.cursor.row + 1..self.size.rows {
                    self.erase_range(row, 0, self.size.cols, selective);
                }
            }
            1 => {
                for row in 0..self.cursor.row {
                    self.erase_range(row, 0, self.size.cols, selective);
                }
                self.erase_range(self.cursor.row, 0, self.cursor.col + 1, selective);
            }
            2 => {
                for row in 0..self.size.rows {
                    self.erase_range(row, 0, self.size.cols, selective);
                }
                if !self.alternate_active && !selective {
                    self.clear_anchor = true;
                }
            }
            3 => {
                self.clear_scrollback();
            }
            _ => {}
        }
        if full_clear && !selective {
            self.effect(GridEffect::Clear {
                alternate: self.alternate_active,
                history_size: self.history_size(),
            });
        }
    }

    pub(super) fn erase_line(&mut self, mode: u16, selective: bool) {
        self.pending_wrap = false;
        let (start, end) = match mode {
            0 => (self.cursor.col, self.size.cols),
            1 => (0, self.cursor.col + 1),
            2 => (0, self.size.cols),
            _ => return,
        };
        self.erase_range(self.cursor.row, start, end, selective);
    }

    pub(super) fn erase_chars(&mut self, count: usize) {
        self.pending_wrap = false;
        self.erase_range(
            self.cursor.row,
            self.cursor.col,
            self.cursor.col.saturating_add(count).min(self.size.cols),
            false,
        );
    }

    /// DECALN: like xterm, reset the margins, home the cursor and fill the
    /// screen with `E`. The fill keeps the pen's colors but no other rendition.
    pub(super) fn screen_alignment_test(&mut self) {
        let old = self.cursor;
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows;
        self.reset_lr_margins();
        let fill = Cell {
            character: 'E',
            style: Style {
                foreground: self.pen.style.foreground,
                background: self.pen.style.background,
                ..Style::default()
            },
            ..Cell::default()
        };
        let cols = self.size.cols;
        for row in &mut self.screen_mut().rows {
            row.discard_packed();
            row.cells.resize(cols, fill);
            row.cells.fill(fill);
            row.extras.clear();
            row.occupied = cols;
            row.wrapped = false;
        }
        // The text under any image is gone, as after ED 2.
        self.effect(GridEffect::Clear {
            alternate: self.alternate_active,
            history_size: self.history_size(),
        });
        self.mark_full_damage();
        self.cursor.row = 0;
        self.cursor.col = 0;
        self.motion_done(old);
    }

    fn repair_wide(row: &mut Row, blank: &Cell) {
        // Alternate-screen resize can move or replace any cell. That uncommon
        // path conservatively invalidates the entire prefix.
        row.occupied = row.cells.len();
        for col in 0..row.cells.len() {
            Self::repair_wide_at(row, col, blank);
        }
    }

    /// Blank `col` if it is half of a broken wide pair. Blanking never breaks
    /// another pair, so callers only need to check the columns they disturbed.
    fn repair_wide_at(row: &mut Row, col: usize, blank: &Cell) {
        let flags = row.cells[col].flags;
        let invalid = (flags & Cell::WIDE != 0
            && (col + 1 == row.cells.len() || row.cells[col + 1].flags & Cell::WIDE_SPACER == 0))
            || (flags & Cell::WIDE_SPACER != 0
                && (col == 0 || row.cells[col - 1].flags & Cell::WIDE == 0))
            || (flags & Cell::LEADING_WIDE_SPACER != 0
                && (!row.wrapped || col + 1 != row.cells.len()));
        if invalid {
            row.blank_cell(col, *blank);
        }
    }

    /// Account for pen blanks ending at `end` after cells shifted right by
    /// `shift`; matching erase blanks leave the occupied bound valid.
    fn edited_occupied(row: &mut Row, shift: usize, end: usize, blank: &Cell) {
        let mut occupied = (row.occupied + shift).min(row.cells.len());
        if blank.style.background != row.clear_background {
            occupied = occupied.max(end);
        }
        row.occupied = occupied;
    }

    /// ICH and DCH shift cells up to the right margin and do nothing when
    /// the cursor is outside the left/right margins, as in xterm.
    pub(super) fn insert_chars(&mut self, count: usize) {
        self.pending_wrap = false;
        if !self.in_lr_margins() {
            return;
        }
        let col = self.cursor.col;
        let row = self.cursor.row;
        let right = self.right_margin;
        let count = count.min(right - col);
        if count == 0 {
            return;
        }
        let blank = self.blank();
        let cols = self.size.cols;
        let active = &mut self.screen_mut().rows[row];
        if active.cells[col].flags & Cell::WIDE_SPACER != 0 {
            Self::clear_wide_at(active, col, &blank);
        }
        Self::clear_straddling_right_margin(active, right, &blank);
        active.cells[col..right].rotate_right(count);
        active.rotate_extras_right(col..right, count);
        active.cells[col..col + count].fill(blank);
        active.clear_extras(col..col + count);
        if right == cols {
            active.wrapped = false;
        }
        // Only a wide glyph pushed against the margin can lose its spacer,
        // and a leading spacer there no longer ends a wrapped row.
        Self::repair_wide_at(active, right - 1, &blank);
        Self::edited_occupied(active, count, col + count, &blank);
        self.mark(row, col.saturating_sub(1), cols);
    }

    /// A wide glyph whose spacer sits right of the margin can't be shifted
    /// by half, so blank both halves first.
    fn clear_straddling_right_margin(row: &mut Row, right: usize, blank: &Cell) {
        if right < row.cells.len() && row.cells[right - 1].flags & Cell::WIDE != 0 {
            Self::clear_wide_at(row, right - 1, blank);
        }
    }

    pub(super) fn delete_chars(&mut self, count: usize) {
        self.pending_wrap = false;
        if !self.in_lr_margins() {
            return;
        }
        let col = self.cursor.col;
        let row = self.cursor.row;
        let right = self.right_margin;
        let count = count.min(right - col);
        if count == 0 {
            return;
        }
        let blank = self.blank();
        let cols = self.size.cols;
        let active = &mut self.screen_mut().rows[row];
        if active.cells[col].flags & Cell::WIDE_SPACER != 0 {
            Self::clear_wide_at(active, col, &blank);
        }
        Self::clear_straddling_right_margin(active, right, &blank);
        active.cells[col..right].rotate_left(count);
        active.rotate_extras_left(col..right, count);
        active.cells[right - count..right].fill(blank);
        active.clear_extras(right - count..right);
        if right == cols {
            active.wrapped = false;
        }
        // A spacer whose glyph was deleted lands at the cursor, and a former
        // last-column leading spacer lands just before the new blanks.
        Self::repair_wide_at(active, col, &blank);
        if count < right - col {
            Self::repair_wide_at(active, right - count - 1, &blank);
        }
        Self::edited_occupied(active, 0, right, &blank);
        self.mark(row, col.saturating_sub(1), cols);
    }

    fn scroll_region_down(&mut self, top: usize, bottom: usize, count: usize) {
        let blank = self.blank();
        let cols = self.size.cols;
        let count = count.min(bottom - top);
        let history_before = self.history_size();
        // The region's last `count` rows move to its top at once, blanked.
        let rows = &mut self.screen_mut().rows;
        rows.rotate_range_right(top..bottom, count);
        for row in rows.range_mut(top..top + count) {
            row.clear(cols, &blank);
        }
        if count != 0 {
            self.mark_scroll_damage(top, bottom, -(count as i32));
            self.effect(GridEffect::Scroll {
                alternate: self.alternate_active,
                top,
                bottom,
                lines: -(count as i64),
                retains_history: false,
                history_before,
                history_after: self.history_size(),
            });
        }
    }

    pub(super) fn scroll_up(&mut self, count: usize) {
        if !self.margins_span_screen() {
            self.scroll_rect(self.scroll_top, self.scroll_bottom, count, true);
            return;
        }
        let retain = !self.alternate_active && self.scroll_top == 0;
        self.scroll_region_up(self.scroll_top, self.scroll_bottom, count, retain);
    }
    pub(super) fn scroll_down(&mut self, count: usize) {
        if !self.margins_span_screen() {
            self.scroll_rect(self.scroll_top, self.scroll_bottom, count, false);
            return;
        }
        self.scroll_region_down(self.scroll_top, self.scroll_bottom, count);
    }
    fn cursor_in_margins(&self) -> bool {
        self.cursor.row >= self.scroll_top
            && self.cursor.row < self.scroll_bottom
            && self.in_lr_margins()
    }
    pub(super) fn insert_lines(&mut self, count: usize) {
        self.pending_wrap = false;
        if self.cursor_in_margins() {
            if self.margins_span_screen() {
                self.scroll_region_down(self.cursor.row, self.scroll_bottom, count);
            } else {
                self.scroll_rect(self.cursor.row, self.scroll_bottom, count, false);
            }
            // Like xterm and the VT510, IL leaves the cursor at the left margin.
            self.carriage_return();
        }
    }
    pub(super) fn delete_lines(&mut self, count: usize) {
        self.pending_wrap = false;
        if self.cursor_in_margins() {
            if self.margins_span_screen() {
                self.scroll_region_up(self.cursor.row, self.scroll_bottom, count, false);
            } else {
                self.scroll_rect(self.cursor.row, self.scroll_bottom, count, true);
            }
            self.carriage_return();
        }
    }
    pub(super) fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let bottom = bottom.min(self.size.rows);
        if top >= bottom || bottom - top < 2 {
            return;
        }
        self.scroll_top = top;
        self.scroll_bottom = bottom;
        self.goto(0, 0);
    }

    pub(super) fn save_cursor(&mut self) {
        let saved = SavedCursor {
            cursor: self.cursor,
            pen: self.pen.clone(),
            pending_wrap: self.pending_wrap,
            origin_mode: self.origin_mode,
            autowrap: self.autowrap,
            valid: true,
        };
        self.screen_mut().saved = saved;
    }

    pub(super) fn has_saved_cursor(&self) -> bool {
        self.screen().saved.valid
    }

    pub(super) fn restore_cursor(&mut self) {
        let old = self.cursor;
        let saved = self.screen().saved.clone();
        self.cursor.row = saved.cursor.row.min(self.size.rows - 1);
        self.cursor.col = saved.cursor.col.min(self.size.cols - 1);
        // The OSC 8 link is not part of the saved rendition: keep the live
        // one and take everything else, including SGR 58, from the save.
        let link = self.pen.extra.take();
        let underline_color = saved.pen.underline_color();
        self.pen = Pen {
            extra: link,
            ..saved.pen
        };
        self.pen.set_underline_color(underline_color);
        self.origin_mode = saved.origin_mode;
        self.autowrap = saved.autowrap;
        self.motion_done(old);
        // A wrap saved at the right margin is still pending there.
        self.pending_wrap = saved.pending_wrap && self.cursor.col + 1 == self.right_limit();
    }

    pub(super) fn set_alternate(&mut self, enabled: bool, clear: bool, save_cursor: bool) {
        if enabled == self.alternate_active {
            return;
        }
        if enabled && save_cursor {
            self.save_cursor();
        }
        let cursor = self.cursor;
        let pending_wrap = self.pending_wrap;
        let previous = self.screen_mut();
        previous.cursor = cursor;
        previous.pending_wrap = pending_wrap;
        if enabled {
            if self.alternate.is_none() {
                self.alternate = Some(Screen::new(self.size));
            }
            if clear {
                let blank = self.blank();
                let alternate = self.alternate.as_mut().expect("allocated alternate screen");
                for row in &mut alternate.rows {
                    row.clear(self.size.cols, &blank);
                }
                alternate.cursor.row = 0;
                alternate.cursor.col = 0;
                alternate.pending_wrap = false;
                self.effect(GridEffect::Clear {
                    alternate: true,
                    history_size: 0,
                });
            }
        }
        self.alternate_active = enabled;
        // Each screen keeps its own position; cursor visibility, shape and
        // blinking stay terminal-wide.
        let position = self.screen().cursor;
        self.cursor.row = position.row;
        self.cursor.col = position.col;
        self.pending_wrap = self.screen().pending_wrap;
        if !enabled && save_cursor {
            self.restore_cursor();
        }
        if !enabled && clear {
            // 1049 clears the alternate screen on every entry, so its rows
            // are dead weight until then. Its images go with the text.
            self.alternate = None;
            self.effect(GridEffect::Clear {
                alternate: true,
                history_size: 0,
            });
        }
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows;
        self.reset_lr_margins();
        self.mark_full_damage();
    }

    pub(super) fn scroll_display(&mut self, delta: i32) -> bool {
        self.release_history_read_cache();
        if self.alternate_active {
            return false;
        }
        let previous = self.display_offset;
        self.display_offset = if delta >= 0 {
            previous
                .saturating_add(delta as usize)
                .min(self.history.len())
        } else {
            previous.saturating_sub(delta.unsigned_abs() as usize)
        };
        if previous != self.display_offset {
            self.mark_full_damage();
            true
        } else {
            false
        }
    }

    pub(super) fn clear_scrollback(&mut self) -> bool {
        if self.alternate_active {
            return false;
        }
        self.release_history_read_cache();
        self.clear_anchor = false;
        let removed = self.history.len();
        let changed = removed != 0 || self.display_offset != 0;
        self.clear_history_rows();
        self.pending_compaction = 0;
        self.display_offset = 0;
        if changed {
            self.mark_full_damage();
            if removed != 0 {
                self.effect(GridEffect::ClearHistory { removed });
            }
        }
        changed
    }

    pub(super) fn set_history_limits(&mut self, limit: usize, byte_limit: usize) {
        if (limit, byte_limit) == (self.history_limit, self.history_byte_limit) {
            return;
        }
        self.release_history_read_cache();
        self.history_limit = limit;
        self.history_byte_limit = byte_limit;
        self.trim_history();
        self.enforce_history_bytes();
        self.mark_full_damage();
    }

    fn trim_history(&mut self) {
        let removed = self.history.len().saturating_sub(self.history_limit);
        while self.history.len() > self.history_limit {
            self.pop_history_front();
        }
        if self.history.capacity() > self.history_limit.max(64).saturating_mul(2) {
            self.history.shrink_to(self.history_limit);
        }
        self.display_offset = self.display_offset.min(self.history.len());
        if removed != 0 {
            self.effect(GridEffect::ClearHistory { removed });
        }
    }

    pub(super) fn soft_reset(&mut self, cursor_shape: CursorShape) {
        let old = self.cursor;
        self.cursor = Cursor {
            row: old.row,
            col: old.col,
            shape: cursor_shape,
            ..Cursor::default()
        };
        self.pen = Pen::default();
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows;
        // A program that dies with DECLRMM set would otherwise turn every
        // later `CSI s` into DECSLRM.
        self.set_lr_margin_mode(false);
        self.autowrap = true;
        self.origin_mode = false;
        self.insert_mode = false;
        self.pending_wrap = false;
        self.end_grapheme();
        self.screen_mut().saved = SavedCursor {
            autowrap: true,
            ..SavedCursor::default()
        };
        if self.cursor != old {
            self.cursor_changed(old);
        }
    }

    pub(super) fn reset(&mut self) {
        self.release_history_read_cache();
        self.alternate_active = false;
        self.alternate = None;
        self.clear_history_rows();
        self.pending_compaction = 0;
        self.display_offset = 0;
        self.cursor = Cursor::default();
        self.pen = Pen::default();
        self.primary.saved = SavedCursor {
            autowrap: true,
            ..SavedCursor::default()
        };
        self.primary.cursor = self.cursor;
        for row in &mut self.primary.rows {
            row.clear(self.size.cols, &Cell::default());
        }
        self.tabs = Self::default_tabs(self.size.cols);
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows;
        self.set_lr_margin_mode(false);
        self.autowrap = true;
        self.origin_mode = false;
        self.insert_mode = false;
        self.newline_mode = false;
        self.pending_wrap = false;
        self.clear_anchor = false;
        self.combining_cache.clear();
        self.mark_full_damage();
        self.effect(GridEffect::Reset);
    }

    /// Resize reconstructs logical lines only when their width changes. Normal
    /// feed, history reads, and height-only resizes never flatten the buffer.
    pub(super) fn resize(&mut self, size: Size) {
        self.release_history_read_cache();
        self.end_grapheme();
        let size = size.clamped();
        if size == self.size {
            return;
        }
        let current_cursor = self.cursor;
        let current_wrap = self.pending_wrap;
        self.screen_mut().cursor = current_cursor;
        self.screen_mut().pending_wrap = current_wrap;
        // DECSET 1049 saves the primary cursor before parking that screen.
        // Keep this saved anchor attached to the same text through reflow and
        // height changes, without replacing an independently saved position.
        let saved_primary_follows_cursor = self.alternate_active
            && self.primary.saved.cursor.row == self.primary.cursor.row
            && self.primary.saved.cursor.col == self.primary.cursor.col
            && self.primary.saved.pending_wrap == self.primary.pending_wrap;
        if size.cols != self.size.cols {
            self.reflow_primary(size);
            if let Some(alternate) = &mut self.alternate {
                // Alternate-screen applications repaint after SIGWINCH. Keep
                // absolute cell positions rather than manufacturing history.
                // Discard shrinking height before widening the remaining rows
                // so the intermediate viewport obeys the same cell budget.
                alternate.rows.truncate(size.rows);
                for row in &mut alternate.rows {
                    row.cells.resize(size.cols, Cell::default());
                    row.clear_extras(size.cols..usize::MAX);
                    row.wrapped = false;
                    Self::repair_wide(row, &Cell::default());
                }
                alternate.cursor.col = alternate.cursor.col.min(size.cols - 1);
                alternate.pending_wrap = false;
            }
        }
        self.resize_height(size);
        if saved_primary_follows_cursor {
            self.primary.saved.cursor.row = self.primary.cursor.row;
            self.primary.saved.cursor.col = self.primary.cursor.col;
            self.primary.saved.pending_wrap = self.primary.pending_wrap;
        }
        let widened = size.cols > self.size.cols;
        self.size = size;
        self.flood_exposed = false;
        // Reflow recounted history; height changes keep its count.
        self.trim_history();
        if widened {
            // History is charged at the new width from here on, but the
            // widen itself evicts nothing the old width kept (as `reflow`
            // charges it), so a transient widen loses no history. Later
            // output, or a resize that doesn't widen, evicts down to the
            // budget.
            self.pack_history_to_budget();
        } else {
            self.enforce_history_bytes();
        }
        self.pending_compaction = self.pending_compaction.min(self.history.len());
        if self.pending_compaction != 0 {
            self.history_activity = true;
        }
        self.scroll_top = 0;
        self.scroll_bottom = size.rows;
        self.reset_lr_margins();
        // Stops past the visible width survive a shrink, so only columns
        // beyond the widest width seen so far get default stops.
        for col in self.tabs.len()..size.cols {
            self.tabs.push(col != 0 && col % 8 == 0);
        }
        self.cursor = self.screen().cursor;
        self.cursor.row = self.cursor.row.min(size.rows - 1);
        self.cursor.col = self.cursor.col.min(size.cols - 1);
        self.pending_wrap = self.screen().pending_wrap && self.cursor.col + 1 == size.cols;
        self.dirty = vec![None; size.rows];
        self.mark_full_damage();
    }

    fn resize_height(&mut self, size: Size) {
        let history_before = self.history.len();
        // Shrink by removing bottom space first, then shift only enough rows
        // into history to keep the cursor visible.
        if self.primary.rows.len() > size.rows {
            let remove_top = self.primary.cursor.row.saturating_sub(size.rows - 1);
            for _ in 0..remove_top {
                if let Some(row) = self.primary.rows.pop_front() {
                    // Compaction walks the newest `pending_compaction` rows,
                    // so new history joins that window instead of being packed
                    // here and hiding older dense rows from the idle steps.
                    self.push_history(row);
                    self.pending_compaction += 1;
                }
            }
            self.primary.cursor.row = self.primary.cursor.row.saturating_sub(remove_top);
            self.primary.rows.truncate(size.rows);
        } else if self.primary.rows.len() < size.rows {
            if !self.clear_anchor {
                while self.primary.rows.len() < size.rows {
                    let Some(mut row) = self.pop_history_back() else {
                        break;
                    };
                    self.pending_compaction = self.pending_compaction.saturating_sub(1);
                    row.make_dense();
                    self.primary.rows.push_front(row);
                    self.primary.cursor.row += 1;
                }
            }
            while self.primary.rows.len() < size.rows {
                self.primary
                    .rows
                    .push_back(Row::new(size.cols, &Cell::default()));
            }
        }
        if let Some(alternate) = &mut self.alternate {
            alternate.rows.truncate(size.rows);
            while alternate.rows.len() < size.rows {
                alternate
                    .rows
                    .push_back(Row::new(size.cols, &Cell::default()));
            }
            alternate.cursor.row = alternate.cursor.row.min(size.rows - 1);
        }
        if self.display_offset != 0 {
            self.display_offset = if self.history.len() >= history_before {
                self.display_offset
                    .saturating_add(self.history.len() - history_before)
            } else {
                self.display_offset
                    .saturating_sub(history_before - self.history.len())
            };
        }
    }
}

#[cfg(test)]
mod tests;
