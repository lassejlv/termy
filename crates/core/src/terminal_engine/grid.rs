//! Row-oriented terminal storage. Scrolling moves row handles, not cells.

use std::{
    collections::VecDeque,
    sync::atomic::{AtomicUsize, Ordering},
};

use super::types::{
    Cell, Color, Cursor, CursorShape, Damage, DirtySpan, GridEffect, Size, Style, ViewportScroll,
};

mod combining;
mod print;
mod row;
use combining::CombiningCache;
use row::PackedCells;

const MAX_HISTORY_ROWS: usize = 20_000;
const MAX_SCROLL_DAMAGE: usize = 32;
const COMPACT_OUTPUT_BURST_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub(super) struct Row {
    cells: Vec<Cell>,
    packed: Option<Box<PackedCells>>,
    // Cells after this prefix still equal the blank used by the last clear.
    // Keeping a conservative upper bound avoids rewriting an entire row when
    // short output lines recycle it through scrollback.
    occupied: usize,
    clear_background: Color,
    pub(super) wrapped: bool,
}

impl Row {
    // The suffix tracked below contains erase blanks: only their background
    // varies. Text, attributes, wide flags and metadata belong in `occupied`.
    fn new(cols: usize, blank: &Cell) -> Self {
        Self {
            cells: vec![blank.clone(); cols],
            packed: None,
            occupied: 0,
            clear_background: blank.style.background,
            wrapped: false,
        }
    }

    fn clear(&mut self, cols: usize, blank: &Cell) {
        self.make_dense();
        let end = if self.clear_background == blank.style.background {
            self.occupied.min(cols)
        } else {
            cols
        };
        self.cells.resize(cols, blank.clone());
        self.cells[..end].fill(blank.clone());
        self.occupied = 0;
        self.clear_background = blank.style.background;
        self.wrapped = false;
    }

    fn content_len(&self) -> usize {
        if self.wrapped {
            return self.cells().len();
        }
        self.cells()
            .iter()
            .rposition(|cell| cell != &Cell::default())
            .map_or(0, |index| index + 1)
    }
}

#[derive(Clone, Default)]
struct SavedCursor {
    cursor: Cursor,
    pen: Cell,
    pending_wrap: bool,
    origin_mode: bool,
    autowrap: bool,
}

struct Screen {
    rows: VecDeque<Row>,
    saved: SavedCursor,
    cursor: Cursor,
    pending_wrap: bool,
}

/// Keep only the rows a resized viewport and its bounded history can retain.
/// Row positions remain absolute while old rows are evicted, so cursor and
/// viewport anchors can be remapped without materializing the entire result.
struct ReflowWindow {
    rows: VecDeque<Row>,
    spare: Option<Row>,
    size: Size,
    limit: usize,
    first: usize,
    produced: usize,
    cursor: Option<(usize, usize, bool)>,
    boundary: Option<usize>,
    viewport: Option<usize>,
    #[cfg(test)]
    row_allocations: usize,
}

impl ReflowWindow {
    fn new(size: Size, history_limit: usize) -> Self {
        Self {
            rows: VecDeque::new(),
            spare: None,
            size,
            limit: history_limit + size.rows,
            first: 0,
            produced: 0,
            cursor: None,
            boundary: None,
            viewport: None,
            #[cfg(test)]
            row_allocations: 0,
        }
    }

    fn new_row(&mut self) -> Row {
        if let Some(mut row) = self.spare.take() {
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

    fn push(&mut self, row: Row) {
        let index = self.produced;
        self.produced += 1;
        if self
            .cursor
            .is_some_and(|(cursor, _, _)| index >= cursor + self.size.rows)
        {
            // Resize discards rows below the cursor's final visible screen.
            // Continue counting them, but reuse their single scratch buffer.
            self.spare = Some(row);
            return;
        }
        if self.rows.len() == self.limit {
            self.spare = self.rows.pop_front();
            self.first += 1;
        }
        self.rows.push_back(row);
    }
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
    history: VecDeque<Row>,
    requested_history_limit: usize,
    history_limit: usize,
    pending_compaction: usize,
    compact_on_scroll: bool,
    output_since_compaction: usize,
    display_offset: usize,
    tabs: Vec<bool>,
    scroll_top: usize,
    scroll_bottom: usize,
    pending_wrap: bool,
    // Erasing the display establishes a boundary that width reflow must not
    // cross when choosing the new live viewport.
    clear_anchor: bool,
    full_damage: bool,
    visual_dirty: bool,
    dirty: Vec<Option<(usize, usize)>>,
    pending_scrolls: Vec<ViewportScroll>,
    combining_cache: CombiningCache,
    grapheme_open: bool,
    grapheme_ordinary: bool,
    // Only meaningful while a grapheme is open. A closed grapheme at the
    // margin marks a wide base dropped there with autowrap disabled.
    grapheme_at_margin: bool,
    pub(super) history_activity: bool,
    // Borrowed history reads usually decode one contiguous viewport. Track
    // that interval so the next mutation does not scan untouched history.
    history_read_start: AtomicUsize,
    history_read_end: AtomicUsize,
    track_effects: bool,
    effects: Vec<GridEffect>,
    #[cfg(test)]
    last_reflow_row_allocations: usize,
    pub(super) cursor: Cursor,
    pub(super) pen: Cell,
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

    /// Pack queued history without switching scrolling rows to immediate
    /// packing, for steps forced while output is still arriving.
    pub(super) fn compact_pending_history(&mut self, limit: usize) {
        self.pending_compaction = self.pending_compaction.min(self.history.len());
        let start = self.history.len() - self.pending_compaction;
        let count = limit.min(self.pending_compaction);
        for row in self.history.iter_mut().skip(start).take(count) {
            row.compact();
        }
        self.pending_compaction -= count;
    }

    pub(super) fn release_history_read_cache(&mut self) {
        if *self.history_read_end.get_mut() == 0 {
            return;
        }
        let start = std::mem::replace(self.history_read_start.get_mut(), usize::MAX);
        let end = std::mem::take(self.history_read_end.get_mut());
        if start < end {
            for row in self.history.range_mut(start..end) {
                row.release_read_cache();
            }
        }
    }

    pub(super) fn new(size: Size, history_limit: usize) -> Self {
        let size = size.clamped();
        Self {
            size,
            primary: Screen::new(size),
            alternate: None,
            alternate_active: false,
            history: VecDeque::new(),
            requested_history_limit: history_limit.min(MAX_HISTORY_ROWS),
            history_limit: Self::bounded_history(size, history_limit),
            pending_compaction: 0,
            compact_on_scroll: false,
            output_since_compaction: 0,
            display_offset: 0,
            tabs: Self::default_tabs(size.cols),
            scroll_top: 0,
            scroll_bottom: size.rows,
            pending_wrap: false,
            clear_anchor: false,
            full_damage: true,
            visual_dirty: false,
            dirty: vec![None; size.rows],
            pending_scrolls: Vec::with_capacity(MAX_SCROLL_DAMAGE),
            combining_cache: CombiningCache::default(),
            grapheme_open: false,
            grapheme_ordinary: true,
            grapheme_at_margin: false,
            history_activity: false,
            history_read_start: AtomicUsize::new(usize::MAX),
            history_read_end: AtomicUsize::new(0),
            track_effects: false,
            effects: Vec::new(),
            #[cfg(test)]
            last_reflow_row_allocations: 0,
            cursor: Cursor::default(),
            pen: Cell::default(),
            autowrap: true,
            origin_mode: false,
            insert_mode: false,
            newline_mode: false,
        }
    }

    fn bounded_history(size: Size, requested: usize) -> usize {
        requested
            .min(MAX_HISTORY_ROWS)
            .min(Size::MAX_CELLS / size.cols)
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

    pub(super) fn row_cells(&self, line: i32) -> Option<&[Cell]> {
        let row = self.row(line)?;
        if line < 0 && row.packed.is_some() {
            let index = self.history.len() - line.unsigned_abs() as usize;
            self.history_read_start.fetch_min(index, Ordering::Relaxed);
            self.history_read_end
                .fetch_max(index + 1, Ordering::Relaxed);
        }
        Some(row.cells())
    }

    pub(super) fn visible_row_cells(&self, row: usize) -> Option<&[Cell]> {
        if row >= self.size.rows {
            return None;
        }
        self.row_cells(row as i32 - self.display_offset() as i32)
    }

    pub(super) fn mark_full_damage(&mut self) {
        self.visual_dirty = true;
        self.full_damage = true;
        self.pending_scrolls.clear();
    }

    fn mark_scroll_damage(&mut self, top: usize, bottom: usize, lines: i32) {
        self.visual_dirty = true;
        if self.full_damage {
            return;
        }
        // While browsing history, scrolling can change both history and live
        // rows in the viewport. A full refresh also covers eviction at its top.
        if self.display_offset() != 0 {
            self.mark_full_damage();
            return;
        }
        let count = lines.unsigned_abs() as usize;
        debug_assert!(count != 0 && count <= bottom - top);
        // The renderer may have painted the old cursor into a cached row. Its
        // old mark must travel with that row before marking its new position.
        self.mark_cursor(self.cursor);
        let exposed = Some((0, self.size.cols));
        if lines > 0 {
            self.dirty[top..bottom].rotate_left(count);
            self.dirty[bottom - count..bottom].fill(exposed);
        } else {
            self.dirty[top..bottom].rotate_right(count);
            self.dirty[top..top + count].fill(exposed);
        }
        self.mark_cursor(self.cursor);
        if let Some(previous) = self.pending_scrolls.last_mut()
            && previous.top == top
            && previous.bottom == bottom
            && previous.lines.signum() == lines.signum()
        {
            // Once the entire region was exposed, every cell is dirty, so a
            // full rotation plus those patches represents any further scroll.
            let count = (previous.lines.unsigned_abs() as usize + count).min(bottom - top);
            previous.lines = count as i32 * lines.signum();
        } else if self.pending_scrolls.len() < MAX_SCROLL_DAMAGE {
            self.pending_scrolls
                .push(ViewportScroll { top, bottom, lines });
        } else {
            self.mark_full_damage();
        }
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

    fn mark(&mut self, row: usize, start: usize, end: usize) {
        self.visual_dirty = true;
        self.mark_damage(row, start, end);
    }

    fn mark_damage(&mut self, row: usize, start: usize, end: usize) {
        if self.full_damage || row >= self.size.rows || start >= end {
            return;
        }
        let row = row.saturating_add(self.display_offset());
        if let Some(dirty) = self.dirty.get_mut(row) {
            let end = end.min(self.size.cols);
            *dirty = Some(match *dirty {
                Some((old_start, old_end)) => (old_start.min(start), old_end.max(end)),
                None => (start, end),
            });
        }
    }

    fn mark_cursor(&mut self, cursor: Cursor) {
        if cursor.visible {
            // Cursor overlays need text damage but do not edit placeholder cells.
            self.mark_damage(cursor.row, cursor.col, cursor.col.saturating_add(1));
        }
    }

    /// Tracks edits independently of damage consumption, including while full
    /// damage is already pending. Graphics consume it to notice placeholder edits.
    pub(super) fn take_visual_dirty(&mut self) -> bool {
        std::mem::take(&mut self.visual_dirty)
    }

    pub(super) fn end_grapheme(&mut self) {
        self.grapheme_open = false;
        self.grapheme_at_margin = false;
    }

    fn motion_done(&mut self, old: Cursor) {
        self.end_grapheme();
        self.pending_wrap = false;
        self.mark_cursor(old);
        self.mark_cursor(self.cursor);
    }

    pub(super) fn cursor_changed(&mut self, old: Cursor) {
        self.mark_cursor(old);
        self.mark_cursor(self.cursor);
    }

    pub(super) fn take_damage(&mut self) -> Damage {
        // Cell-only consumers cannot replay row rotations. Expand the affected
        // regions so their usual span patching still reconstructs the viewport.
        for scroll in &self.pending_scrolls {
            self.dirty[scroll.top..scroll.bottom].fill(Some((0, self.size.cols)));
        }
        self.pending_scrolls.clear();
        self.take_cell_damage()
    }

    pub(super) fn take_render_damage(&mut self) -> (Damage, Vec<ViewportScroll>) {
        let scrolls = self.pending_scrolls.clone();
        self.pending_scrolls.clear();
        (self.take_cell_damage(), scrolls)
    }

    fn take_cell_damage(&mut self) -> Damage {
        if std::mem::take(&mut self.full_damage) {
            self.dirty.fill(None);
            return Damage::Full;
        }
        let spans = self
            .dirty
            .iter_mut()
            .enumerate()
            .filter_map(|(row, dirty)| {
                dirty
                    .take()
                    .map(|(start, end)| DirtySpan { row, start, end })
            })
            .collect();
        Damage::Partial(spans)
    }

    fn blank(&self) -> Cell {
        Cell {
            style: Style {
                background: self.pen.style.background,
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
            row.cells[start] = blank.clone();
        }
        if row.cells[col].flags & Cell::WIDE != 0 && end < row.cells.len() {
            row.cells[end] = blank.clone();
            end += 1;
        }
        row.cells[col] = blank.clone();
        row.occupied = row.occupied.max(end);
        (start, end)
    }

    pub(super) fn carriage_return(&mut self) {
        let old = self.cursor;
        self.cursor.col = 0;
        self.motion_done(old);
    }
    pub(super) fn backspace(&mut self) {
        let old = self.cursor;
        self.cursor.col = self.cursor.col.saturating_sub(1);
        self.motion_done(old);
    }

    pub(super) fn linefeed(&mut self) {
        let old = self.cursor;
        if self.cursor.row + 1 == self.scroll_bottom {
            self.scroll_up(1);
        } else {
            self.cursor.row = (self.cursor.row + 1).min(self.size.rows - 1);
        }
        if self.newline_mode {
            self.cursor.col = 0;
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
            self.scroll_down(1);
        } else {
            self.cursor.row = self.cursor.row.saturating_sub(1);
        }
        self.motion_done(old);
    }

    pub(super) fn goto(&mut self, row: usize, col: usize) {
        let old = self.cursor;
        self.cursor.row = if self.origin_mode {
            row.saturating_add(self.scroll_top)
                .min(self.scroll_bottom - 1)
        } else {
            row.min(self.size.rows - 1)
        };
        self.cursor.col = col.min(self.size.cols - 1);
        self.motion_done(old);
    }

    pub(super) fn move_cursor(&mut self, row_delta: isize, col_delta: isize) {
        let old = self.cursor;
        let (top, bottom) = if self.origin_mode
            || (self.cursor.row >= self.scroll_top && self.cursor.row < self.scroll_bottom)
        {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, self.size.rows)
        };
        self.cursor.row = self
            .cursor
            .row
            .saturating_add_signed(row_delta)
            .clamp(top, bottom - 1);
        self.cursor.col = self
            .cursor
            .col
            .saturating_add_signed(col_delta)
            .min(self.size.cols - 1);
        self.motion_done(old);
    }

    pub(super) fn tab(&mut self) {
        let old = self.cursor;
        self.cursor.col = ((self.cursor.col + 1)..self.size.cols)
            .find(|&col| self.tabs[col])
            .unwrap_or(self.size.cols - 1);
        self.motion_done(old);
    }
    pub(super) fn backtab(&mut self) {
        let old = self.cursor;
        self.cursor.col = (0..self.cursor.col)
            .rev()
            .find(|&col| self.tabs[col])
            .unwrap_or(0);
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
            let (first_left, first_right) = Self::clear_wide_at(active, start, &blank);
            let (last_left, last_right) = Self::clear_wide_at(active, end - 1, &blank);
            active.cells[start..end].fill(blank);
            if end == active.cells.len() {
                active.wrapped = false;
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

    fn repair_wide(row: &mut Row, blank: &Cell) {
        // Editing and alternate-screen resize can move or replace any cell.
        // Those uncommon paths conservatively invalidate the entire prefix.
        row.occupied = row.cells.len();
        for col in 0..row.cells.len() {
            let flags = row.cells[col].flags;
            let invalid = (flags & Cell::WIDE != 0
                && (col + 1 == row.cells.len()
                    || row.cells[col + 1].flags & Cell::WIDE_SPACER == 0))
                || (flags & Cell::WIDE_SPACER != 0
                    && (col == 0 || row.cells[col - 1].flags & Cell::WIDE == 0))
                || (flags & Cell::LEADING_WIDE_SPACER != 0
                    && (!row.wrapped || col + 1 != row.cells.len()));
            if invalid {
                row.cells[col] = blank.clone();
            }
        }
    }

    pub(super) fn insert_chars(&mut self, count: usize) {
        self.pending_wrap = false;
        let col = self.cursor.col;
        let row = self.cursor.row;
        let count = count.min(self.size.cols - col);
        if count == 0 {
            return;
        }
        let blank = self.blank();
        let cols = self.size.cols;
        let active = &mut self.screen_mut().rows[row];
        if active.cells[col].flags & Cell::WIDE_SPACER != 0 {
            Self::clear_wide_at(active, col, &blank);
        }
        active.cells[col..].rotate_right(count);
        active.cells[col..col + count].fill(blank.clone());
        active.wrapped = false;
        Self::repair_wide(active, &blank);
        self.mark(row, col.saturating_sub(1), cols);
    }

    pub(super) fn delete_chars(&mut self, count: usize) {
        self.pending_wrap = false;
        let col = self.cursor.col;
        let row = self.cursor.row;
        let count = count.min(self.size.cols - col);
        if count == 0 {
            return;
        }
        let blank = self.blank();
        let cols = self.size.cols;
        let active = &mut self.screen_mut().rows[row];
        if active.cells[col].flags & Cell::WIDE_SPACER != 0 {
            Self::clear_wide_at(active, col, &blank);
        }
        active.cells[col..].rotate_left(count);
        active.cells[cols - count..].fill(blank.clone());
        active.wrapped = false;
        Self::repair_wide(active, &blank);
        self.mark(row, col.saturating_sub(1), cols);
    }

    fn scroll_region_up(&mut self, top: usize, bottom: usize, count: usize, retain_history: bool) {
        let blank = self.blank();
        let cols = self.size.cols;
        let count = count.min(bottom - top);
        let history_before = self.history_size();
        for _ in 0..count {
            let removed = self
                .screen_mut()
                .rows
                .remove(top)
                .expect("scroll region row");
            let mut recycled = if retain_history && self.history_limit != 0 {
                let recycled = if self.history.len() == self.history_limit {
                    self.history.pop_front()
                } else {
                    None
                };
                let (retained, recycled) = if self.compact_on_scroll {
                    removed.retain(recycled, cols, &blank)
                } else {
                    (removed, recycled.unwrap_or_else(|| Row::new(cols, &blank)))
                };
                self.history.push_back(retained);
                self.history_activity = true;
                self.pending_compaction = (self.pending_compaction + 1).min(self.history.len());
                if self.display_offset != 0 {
                    self.display_offset = (self.display_offset + 1).min(self.history.len());
                }
                recycled
            } else {
                removed
            };
            recycled.clear(cols, &blank);
            self.screen_mut().rows.insert(bottom - 1, recycled);
        }
        if count != 0 {
            self.mark_scroll_damage(top, bottom, count as i32);
            self.effect(GridEffect::Scroll {
                alternate: self.alternate_active,
                top,
                bottom,
                lines: count as i64,
                retains_history: retain_history && self.history_limit != 0,
                history_before,
                history_after: self.history_size(),
            });
        }
    }

    fn scroll_region_down(&mut self, top: usize, bottom: usize, count: usize) {
        let blank = self.blank();
        let cols = self.size.cols;
        let count = count.min(bottom - top);
        let history_before = self.history_size();
        for _ in 0..count {
            let mut row = self
                .screen_mut()
                .rows
                .remove(bottom - 1)
                .expect("scroll region row");
            row.clear(cols, &blank);
            self.screen_mut().rows.insert(top, row);
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
        let retain = !self.alternate_active && self.scroll_top == 0;
        self.scroll_region_up(self.scroll_top, self.scroll_bottom, count, retain);
    }
    pub(super) fn scroll_down(&mut self, count: usize) {
        self.scroll_region_down(self.scroll_top, self.scroll_bottom, count);
    }
    pub(super) fn insert_lines(&mut self, count: usize) {
        self.pending_wrap = false;
        if self.cursor.row >= self.scroll_top && self.cursor.row < self.scroll_bottom {
            self.scroll_region_down(self.cursor.row, self.scroll_bottom, count);
        }
    }
    pub(super) fn delete_lines(&mut self, count: usize) {
        self.pending_wrap = false;
        if self.cursor.row >= self.scroll_top && self.cursor.row < self.scroll_bottom {
            self.scroll_region_up(self.cursor.row, self.scroll_bottom, count, false);
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
        };
        self.screen_mut().saved = saved;
    }

    pub(super) fn restore_cursor(&mut self) {
        let old = self.cursor;
        let saved = self.screen().saved.clone();
        self.cursor = saved.cursor;
        self.cursor.row = self.cursor.row.min(self.size.rows - 1);
        self.cursor.col = self.cursor.col.min(self.size.cols - 1);
        self.pen = saved.pen;
        self.origin_mode = saved.origin_mode;
        self.autowrap = saved.autowrap;
        self.motion_done(old);
        self.pending_wrap = saved.pending_wrap && self.cursor.col + 1 == self.size.cols;
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
                alternate.cursor = Cursor {
                    shape: self.cursor.shape,
                    blinking: self.cursor.blinking,
                    visible: self.cursor.visible,
                    ..Cursor::default()
                };
                alternate.pending_wrap = false;
                self.effect(GridEffect::Clear {
                    alternate: true,
                    history_size: 0,
                });
            }
        }
        self.alternate_active = enabled;
        self.cursor = self.screen().cursor;
        self.pending_wrap = self.screen().pending_wrap;
        if !enabled && save_cursor {
            self.restore_cursor();
        }
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows;
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
        self.history.clear();
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

    pub(super) fn set_history_limit(&mut self, limit: usize) {
        self.release_history_read_cache();
        self.requested_history_limit = limit.min(MAX_HISTORY_ROWS);
        self.history_limit = Self::bounded_history(self.size, limit);
        self.trim_history();
        self.mark_full_damage();
    }

    fn trim_history(&mut self) {
        let removed = self.history.len().saturating_sub(self.history_limit);
        while self.history.len() > self.history_limit {
            self.history.pop_front();
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
        self.pen = Cell::default();
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows;
        self.autowrap = true;
        self.origin_mode = false;
        self.insert_mode = false;
        self.pending_wrap = false;
        self.end_grapheme();
        self.screen_mut().saved = SavedCursor {
            cursor: Cursor {
                shape: cursor_shape,
                ..Cursor::default()
            },
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
        self.history.clear();
        self.pending_compaction = 0;
        self.display_offset = 0;
        self.cursor = Cursor::default();
        self.pen = Cell::default();
        self.primary.saved = SavedCursor {
            autowrap: true,
            ..SavedCursor::default()
        };
        self.primary.cursor = self.cursor;
        for row in &mut self.primary.rows {
            row.clear(self.size.cols, &self.pen);
        }
        self.tabs = Self::default_tabs(self.size.cols);
        self.scroll_top = 0;
        self.scroll_bottom = self.size.rows;
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
        self.size = size;
        self.history_limit = Self::bounded_history(size, self.requested_history_limit);
        self.trim_history();
        self.pending_compaction = self.pending_compaction.min(self.history.len());
        if self.pending_compaction != 0 {
            self.history_activity = true;
        }
        self.scroll_top = 0;
        self.scroll_bottom = size.rows;
        let old_cols = self.tabs.len();
        self.tabs.resize(size.cols, false);
        for col in old_cols..size.cols {
            self.tabs[col] = col != 0 && col % 8 == 0;
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
                    self.history.push_back(row);
                    self.pending_compaction += 1;
                }
            }
            self.primary.cursor.row = self.primary.cursor.row.saturating_sub(remove_top);
            self.primary.rows.truncate(size.rows);
        } else if self.primary.rows.len() < size.rows {
            if !self.clear_anchor {
                while self.primary.rows.len() < size.rows {
                    let Some(mut row) = self.history.pop_back() else {
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

    fn reflow_primary(&mut self, size: Size) {
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
            Self::bounded_history(size, self.requested_history_limit),
        );
        let mut logical = Vec::new();
        let mut logical_cursor = None;
        let mut logical_boundary = None;
        let mut logical_viewport = None;
        for (index, mut row) in source.enumerate() {
            row.make_dense();
            if index == old_history {
                logical_boundary = Some(logical.len());
            }
            if Some(index) == viewport_source {
                logical_viewport = Some(logical.len());
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
            logical.extend(
                row.cells
                    .into_iter()
                    .take(len)
                    .filter(|cell| cell.flags & Cell::LEADING_WIDE_SPACER == 0),
            );
            if !row.wrapped {
                Self::wrap_logical(
                    std::mem::take(&mut logical),
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
                logical,
                &mut output,
                logical_cursor,
                logical_boundary,
                logical_viewport,
                cursor_pending,
            );
        }
        let cursor = output.cursor.expect("reflow maps the live cursor");
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
        cells: Vec<Cell>,
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
        let mut input = cells.into_iter().enumerate().peekable();
        while let Some((index, mut cell)) = input.next() {
            // A one-column screen must keep making progress, so wide glyphs
            // temporarily occupy one cell there. Recover their second cell
            // when the viewport grows again rather than permanently narrowing
            // the saved text.
            let expanded_wide =
                cols > 1 && cell.flags & Cell::WIDE == 0 && print::cell_width(&cell) == 2;
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
                row.cells[col + 1] = Cell {
                    character: ' ',
                    style: cell.style,
                    flags: Cell::WIDE_SPACER,
                    extra: None,
                };
            }
            row.cells[col] = cell;
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
        output.push(row);
    }
}

#[cfg(test)]
mod tests;
