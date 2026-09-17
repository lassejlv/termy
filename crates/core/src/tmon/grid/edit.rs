use super::view::*;
use super::*;

impl Grid {
    pub(crate) fn new(
        cols: u16,
        rows: u16,
        history_limit: usize,
        default_cursor_style: CursorStyle,
    ) -> Self {
        let cols = usize::from(cols.clamp(1, MAX_GRID_DIMENSION));
        let rows = usize::from(rows.clamp(1, MAX_GRID_DIMENSION));
        Self {
            primary: Screen::new(cols, rows),
            alternate: None,
            alternate_active: false,
            history: VecDeque::new(),
            history_limit: history_limit.min(MAX_SCROLLBACK_LINES),
            display_offset: 0,
            resize_anchor_suppressed_after_clear: false,
            damage: Damage::new(rows),
            cursor_visible: true,
            cursor_style: default_cursor_style,
            default_cursor_style,
            cursor_style_explicit: false,
            cursor_shape_status: cursor_shape_status(default_cursor_style),
            auto_wrap: true,
            insert_mode: false,
            origin_mode: false,
            line_feed_new_line: false,
            bracketed_paste: false,
            cursor_blinking: false,
            focus_reporting: false,
            alternate_scroll: true,
            urgency_hints: true,
            mouse_mode: MouseMode::default(),
            keyboard_mode: KeyboardMode::default(),
            kitty_keyboard_stack: Vec::with_capacity(8),
            inactive_kitty_keyboard_stack: Vec::with_capacity(8),
            tab_stops: default_tab_stops(cols),
            cell_width: 1.0,
            cell_height: 1.0,
            palette: Palette::default(),
            hyperlinks: HashMap::new(),
            hyperlink_identities: HashMap::new(),
            next_hyperlink_id: 1,
            next_hyperlink_prune_len: HYPERLINK_PRUNE_MIN_LEN,
            extras: HashMap::new(),
            next_extra_id: 1,
            effects: VecDeque::with_capacity(8),
        }
    }

    pub(super) fn active(&self) -> &Screen {
        if self.alternate_active {
            self.alternate
                .as_ref()
                .expect("the active alternate screen is initialized")
        } else {
            &self.primary
        }
    }

    pub(super) fn active_mut(&mut self) -> &mut Screen {
        if self.alternate_active {
            let screen = self
                .alternate
                .as_mut()
                .expect("the active alternate screen is initialized");
            screen.plain_ascii_cells = false;
            screen
        } else {
            &mut self.primary
        }
    }

    pub(super) fn pen(&self) -> Pen {
        self.active().pen
    }

    pub(super) fn pen_mut(&mut self) -> &mut Pen {
        &mut self.active_mut().pen
    }

    pub(crate) fn charset(&self, index: usize) -> Charset {
        self.active().charsets[index.min(3)]
    }

    pub(crate) fn set_charset(&mut self, index: usize, charset: Charset) {
        if let Some(target) = self.active_mut().charsets.get_mut(index) {
            *target = charset;
        }
    }

    pub(crate) fn cols(&self) -> usize {
        self.active().cols
    }

    pub(crate) fn rows(&self) -> usize {
        self.active().rows
    }

    pub(crate) fn cursor_position(&self) -> (usize, usize) {
        let screen = self.active();
        (screen.cursor_col, screen.cursor_row)
    }

    pub(crate) fn render_cursor_position(&self) -> (usize, usize) {
        let screen = self.active();
        let col = if screen.row(screen.cursor_row)[screen.cursor_col].wide_spacer() {
            screen.cursor_col.saturating_sub(1)
        } else {
            screen.cursor_col
        };
        (col, screen.cursor_row)
    }

    pub(crate) fn cursor_state(&self) -> Option<CursorState> {
        self.cursor_visible.then(|| {
            let (col, row) = self.render_cursor_position();
            CursorState {
                col,
                row,
                style: self.cursor_style,
            }
        })
    }

    pub(crate) fn display_offset(&self) -> usize {
        if self.alternate_active {
            0
        } else {
            self.display_offset
        }
    }

    pub(crate) fn history_size(&self) -> usize {
        if self.alternate_active {
            0
        } else {
            self.history.len()
        }
    }

    pub(crate) fn bracketed_paste_mode(&self) -> bool {
        self.bracketed_paste
    }

    pub(crate) fn mouse_mode(&self) -> MouseMode {
        self.mouse_mode
    }

    pub(crate) fn keyboard_mode(&self) -> KeyboardMode {
        self.keyboard_mode
    }

    pub(crate) fn alternate_screen_mode(&self) -> bool {
        self.alternate_active
    }

    pub(crate) fn carriage_return(&mut self) {
        let screen = self.active_mut();
        screen.cursor_col = 0;
        screen.wrap_pending = false;
    }

    pub(crate) fn backspace(&mut self) {
        let screen = self.active_mut();
        if screen.cursor_col > 0 {
            screen.cursor_col -= 1;
            screen.wrap_pending = false;
        }
    }

    pub(crate) fn tab(&mut self) {
        if self.active().wrap_pending {
            if !self.auto_wrap {
                return;
            }
            let (row, col) = {
                let screen = self.active();
                (screen.cursor_row, screen.cursor_col)
            };
            self.active_mut().row_mut_through(row, col + 1)[col].set_wrapped(true);
            self.damage.mark(row, col, col);
            self.carriage_return();
            self.line_feed();
            return;
        }

        let (row, col) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col)
        };
        if self.active().row(row)[col].character == ' ' {
            self.active_mut().row_mut_through(row, col + 1)[col].character = '\t';
            self.damage.mark(row, col, col);
        }
        self.move_forward_tabs(1);
    }

    pub(crate) fn move_forward_tabs(&mut self, count: usize) {
        let mut col = self.active().cursor_col;
        for _ in 0..count {
            col = self
                .tab_stops
                .iter()
                .enumerate()
                .skip(col.saturating_add(1))
                .find_map(|(index, stop)| stop.then_some(index))
                .unwrap_or_else(|| self.cols().saturating_sub(1));
        }
        self.active_mut().cursor_col = col;
    }

    pub(crate) fn move_backward_tabs(&mut self, count: usize) {
        let mut col = self.active().cursor_col;
        for _ in 0..count {
            col = self
                .tab_stops
                .iter()
                .enumerate()
                .take(col)
                .rev()
                .find_map(|(index, stop)| stop.then_some(index))
                .unwrap_or(col);
        }
        self.active_mut().cursor_col = col;
    }

    pub(crate) fn set_tab_stop(&mut self) {
        let col = self.active().cursor_col;
        if let Some(stop) = self.tab_stops.get_mut(col) {
            *stop = true;
        }
    }

    pub(crate) fn clear_tab_stop(&mut self, mode: u16) {
        match mode {
            0 => {
                let col = self.active().cursor_col;
                if let Some(stop) = self.tab_stops.get_mut(col) {
                    *stop = false;
                }
            }
            3 => self.tab_stops.fill(false),
            _ => {}
        }
    }

    pub(crate) fn reset_tab_stops(&mut self) {
        self.tab_stops = default_tab_stops(self.cols());
    }

    pub(crate) fn line_feed(&mut self) {
        let (cursor_row, scroll_bottom) = {
            let screen = self.active();
            (screen.cursor_row, screen.scroll_bottom)
        };
        if cursor_row == scroll_bottom {
            self.scroll_up(1);
        } else {
            let screen = self.active_mut();
            screen.cursor_row = (screen.cursor_row + 1).min(screen.rows.saturating_sub(1));
        }
    }

    pub(crate) fn next_line(&mut self) {
        self.carriage_return();
        self.line_feed();
    }

    pub(crate) fn reverse_index(&mut self) {
        let (cursor_row, scroll_top) = {
            let screen = self.active();
            (screen.cursor_row, screen.scroll_top)
        };
        if cursor_row == scroll_top {
            self.scroll_down(1);
        } else {
            let screen = self.active_mut();
            screen.cursor_row = screen.cursor_row.saturating_sub(1);
        }
    }

    pub(crate) fn put_ascii_run(&mut self, bytes: &[u8]) -> usize {
        if bytes.is_empty() || self.insert_mode {
            return 0;
        }

        let (row, col, cols, run_len) = {
            let screen = self.active();
            if screen.wrap_pending {
                return 0;
            }
            let row = screen.cursor_row;
            let col = screen.cursor_col;
            let run_len = bytes.len().min(screen.cols.saturating_sub(col));
            let run_len = screen.row(row)[col..col + run_len]
                .iter()
                .position(|cell| {
                    cell.wide_spacer()
                        || cell.leading_wide_spacer()
                        || character_width(cell.character) > 1
                })
                .unwrap_or(run_len);
            (row, col, screen.cols, run_len)
        };
        if run_len == 0 {
            return 0;
        }
        debug_assert!(
            bytes[..run_len]
                .iter()
                .all(|byte| (0x20..=0x7e).contains(byte))
        );

        let pen = self.pen();
        for (cell, byte) in self.active_mut().row_mut_through(row, col + run_len)
            [col..col + run_len]
            .iter_mut()
            .zip(&bytes[..run_len])
        {
            *cell = pen.cell(char::from(*byte));
        }

        let end = col + run_len;
        let screen = self.active_mut();
        if end >= cols {
            screen.cursor_col = cols.saturating_sub(1);
            screen.wrap_pending = true;
        } else {
            screen.cursor_col = end;
            screen.wrap_pending = false;
        }
        self.damage.mark(row, col, end - 1);
        run_len
    }

    pub(crate) fn put_char(&mut self, character: char) {
        let reported_width = character_width(character);
        if reported_width == 0 {
            self.put_combining(character);
            return;
        }
        // Alacritty uses the Unicode scalar width for insert-mode shifting,
        // but renders every width greater than one as a two-cell glyph. Unicode
        // 15.1 has one width-3 scalar (U+17D8), so keep those values separate.
        let width = if reported_width == 1 { 1 } else { 2 };

        let (wrap_pending, wide_does_not_fit) = {
            let screen = self.active();
            (
                screen.wrap_pending,
                width == 2 && screen.cursor_col + 1 >= screen.cols,
            )
        };
        let should_wrap = wrap_pending || wide_does_not_fit;
        let wrapped_wide_placeholder = wide_does_not_fit && !wrap_pending && self.auto_wrap;
        if should_wrap && self.auto_wrap {
            let (row, col) = {
                let screen = self.active();
                (screen.cursor_row, screen.cursor_col)
            };
            if wide_does_not_fit && !wrap_pending {
                let pen = self.pen();
                let mut spacer = pen.cell(' ');
                spacer.set_leading_wide_spacer(true);
                self.write_cell_at(row, col, spacer);
            }
            self.active_mut().row_mut_through(row, col + 1)[col].set_wrapped(true);
            self.damage.mark(row, col, col);
            self.carriage_return();
            self.line_feed();
        } else if width == 2 && self.active().cursor_col + 1 >= self.active().cols {
            self.active_mut().wrap_pending = true;
            return;
        }

        let pen = self.pen();
        let insert_mode = self.insert_mode && !wrapped_wide_placeholder;
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };

        let shifted_for_insert = insert_mode && col + reported_width < cols;
        if shifted_for_insert {
            let row_cells = self.active_mut().row_mut(row);
            for source in (col..cols - reported_width).rev() {
                row_cells.swap(source + reported_width, source);
            }
        }

        self.write_cell_at(row, col, pen.cell(character));
        if width == 2 && col + 1 < cols {
            let mut spacer = pen.cell(' ');
            spacer.set_wide_spacer(true);
            self.write_cell_at(row, col + 1, spacer);
        }

        {
            let screen = self.active_mut();
            if col + width >= cols {
                screen.cursor_col = cols.saturating_sub(1);
                screen.wrap_pending = true;
            } else {
                screen.cursor_col += width;
                screen.wrap_pending = false;
            }
        }
        let damage_end = if shifted_for_insert {
            cols - 1
        } else {
            col.saturating_add(width - 1).min(cols - 1)
        };
        self.damage.mark(row, col, damage_end);
    }

    pub(super) fn put_combining(&mut self, character: char) {
        let (row, mut col, wrap_pending) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.wrap_pending)
        };
        if !wrap_pending {
            col = col.saturating_sub(1);
        }
        if self.active().row(row)[col].wide_spacer() {
            col = col.saturating_sub(1);
        }

        let cell = self.active().row(row)[col];
        let hyperlink_id = self.cell_hyperlink_id(&cell);
        if cell.has_combining()
            && let Some(metadata_id) = cell.metadata_id.filter(|id| is_pooled_extra_id(*id))
        {
            // Pooled extras have exactly one live cell owner. Grid edits and
            // reflow move cells rather than duplicating them, so appending in
            // place is safe and avoids retaining every previous text prefix.
            self.debug_assert_extra_unique(metadata_id);
            let extra = self
                .extras
                .get_mut(&metadata_id)
                .expect("pooled cell metadata always resolves");
            debug_assert_eq!(extra.hyperlink_id, hyperlink_id);
            extra.combining.push(character);
            self.damage.mark(row, col, col);
            return;
        }

        let metadata_id = if !cell.has_combining() && hyperlink_id.is_none() {
            inline_combining_metadata_id(character)
        } else {
            let mut combining = if cell.has_combining() {
                let previous_id = cell
                    .metadata_id
                    .expect("cells with combining text always carry metadata");
                inline_combining_character(previous_id).map_or_else(
                    || {
                        self.extras
                            .get(&previous_id)
                            .map(|extra| extra.combining.clone())
                            .expect("pooled cell metadata always resolves")
                    },
                    CombiningText::from_char,
                )
            } else {
                CombiningText::from_char(character)
            };
            if cell.has_combining() {
                combining.push(character);
            }
            self.insert_extra(CellExtra {
                combining,
                hyperlink_id,
            })
        };
        let cell = &mut self.active_mut().row_mut_through(row, col + 1)[col];
        cell.metadata_id = Some(metadata_id);
        cell.set_has_combining(true);
        self.damage.mark(row, col, col);
    }

    #[cfg(debug_assertions)]
    pub(super) fn debug_assert_extra_unique(&self, id: NonZeroU32) {
        let screen_references = |screen: &Screen| {
            screen
                .cells
                .iter()
                .flatten()
                .filter(|cell| cell.metadata_id == Some(id))
                .count()
        };
        let references = screen_references(&self.primary)
            + self.alternate.as_ref().map_or(0, screen_references)
            + self
                .history
                .iter()
                .flat_map(HistoryRow::iter)
                .filter(|cell| cell.metadata_id == Some(id))
                .count();
        debug_assert_eq!(
            references, 1,
            "pooled cell metadata must have exactly one live owner"
        );
    }

    #[cfg(not(debug_assertions))]
    pub(super) fn debug_assert_extra_unique(&self, _id: NonZeroU32) {}

    pub(super) fn insert_extra(&mut self, extra: CellExtra) -> NonZeroU32 {
        if self.extras.len() >= EXTRA_PRUNE_INTERVAL as usize
            && self.next_extra_id.is_multiple_of(EXTRA_PRUNE_INTERVAL)
        {
            self.prune_extras();
        }
        let id = self.next_available_extra_id();
        self.extras.insert(id, extra);
        id
    }

    pub(super) fn next_available_extra_id(&mut self) -> NonZeroU32 {
        loop {
            let raw = self.next_extra_id.clamp(1, METADATA_VALUE_MASK);
            self.next_extra_id = if raw == METADATA_VALUE_MASK {
                1
            } else {
                raw + 1
            };
            let id = pooled_extra_metadata_id(raw);
            if !self.extras.contains_key(&id) {
                return id;
            }
        }
    }

    pub(super) fn prune_extras(&mut self) {
        let mut live = HashSet::new();
        live.extend(
            self.primary
                .cells
                .iter()
                .flatten()
                .filter_map(|cell| cell.metadata_id.filter(|id| is_pooled_extra_id(*id))),
        );
        if let Some(alternate) = &self.alternate {
            live.extend(
                alternate
                    .cells
                    .iter()
                    .flatten()
                    .filter_map(|cell| cell.metadata_id.filter(|id| is_pooled_extra_id(*id))),
            );
        }
        live.extend(self.history.iter().flat_map(|row| {
            row.iter()
                .filter_map(|cell| cell.metadata_id.filter(|id| is_pooled_extra_id(*id)))
        }));
        self.extras.retain(|id, _| live.contains(id));
    }

    pub(super) fn write_cell_at(&mut self, row: usize, col: usize, cell: Cell) {
        let old = self.active().row(row)[col];
        let old_width = character_width(old.character);
        let overwrites_wide = old.wide_spacer() || old_width > 1;
        if overwrites_wide && col <= 1 {
            self.clear_leading_wide_spacer_before(row);
        }

        let cleared_neighbor = if old_width > 1 && col + 1 < self.cols() {
            self.active_mut().row_mut_through(row, col + 2)[col + 1].set_wide_spacer(false);
            Some(col + 1)
        } else if old.wide_spacer() && col > 0 {
            self.clear_cell_combining(row, col - 1);
            self.active_mut().row_mut_through(row, col)[col - 1].character = ' ';
            Some(col - 1)
        } else {
            None
        };
        self.active_mut().row_mut_through(row, col + 1)[col] = cell;
        if let Some(neighbor) = cleared_neighbor {
            self.damage.mark(row, neighbor, neighbor);
        }
    }

    pub(super) fn clear_cell_combining(&mut self, row: usize, col: usize) {
        let cell = self.active().row(row)[col];
        if !cell.has_combining() {
            return;
        }
        let hyperlink_id = self.cell_hyperlink_id(&cell);
        let cell = &mut self.active_mut().row_mut_through(row, col + 1)[col];
        cell.metadata_id = hyperlink_id.map(inline_hyperlink_metadata_id);
        cell.set_has_combining(false);
    }

    pub(super) fn clear_leading_wide_spacer_before(&mut self, row: usize) {
        let col = self.cols().saturating_sub(1);
        let changed = if row > 0 {
            let previous = self.active_mut().row_mut_through(row - 1, col + 1);
            previous[col].take_leading_wide_spacer()
        } else if !self.alternate_active {
            self.history
                .back_mut()
                .is_some_and(|previous| previous.take_leading_wide_spacer(col))
        } else {
            false
        };
        if changed {
            if row > 0 {
                self.damage.mark(row - 1, col, col);
            } else {
                self.damage.mark_full();
            }
        }
    }

    pub(crate) fn cursor_up(&mut self, count: usize) {
        let origin_mode = self.origin_mode;
        let origin_offset = if origin_mode {
            self.active().scroll_top
        } else {
            0
        };
        let screen = self.active_mut();
        let target = screen.cursor_row as i64 - count as i64 + origin_offset as i64;
        let maximum = if origin_mode {
            screen.scroll_bottom
        } else {
            screen.rows.saturating_sub(1)
        };
        screen.cursor_row = target.clamp(0, maximum as i64) as usize;
        screen.wrap_pending = false;
    }

    pub(crate) fn cursor_down(&mut self, count: usize) {
        let origin_mode = self.origin_mode;
        let origin_offset = if origin_mode {
            self.active().scroll_top
        } else {
            0
        };
        let screen = self.active_mut();
        let maximum = if origin_mode {
            screen.scroll_bottom
        } else {
            screen.rows.saturating_sub(1)
        };
        screen.cursor_row = screen
            .cursor_row
            .saturating_add(count)
            .saturating_add(origin_offset)
            .min(maximum);
        screen.wrap_pending = false;
    }

    pub(crate) fn cursor_forward(&mut self, count: usize) {
        let screen = self.active_mut();
        screen.cursor_col = screen
            .cursor_col
            .saturating_add(count)
            .min(screen.cols.saturating_sub(1));
        screen.wrap_pending = false;
    }

    pub(crate) fn cursor_backward(&mut self, count: usize) {
        let screen = self.active_mut();
        screen.cursor_col = screen.cursor_col.saturating_sub(count);
        screen.wrap_pending = false;
    }

    pub(crate) fn cursor_next_line(&mut self, count: usize) {
        self.cursor_down(count);
        self.carriage_return();
    }

    pub(crate) fn cursor_previous_line(&mut self, count: usize) {
        self.cursor_up(count);
        self.carriage_return();
    }

    pub(crate) fn set_cursor_col(&mut self, col: usize) {
        let origin_mode = self.origin_mode;
        let screen = self.active_mut();
        if origin_mode {
            screen.cursor_row = screen
                .cursor_row
                .saturating_add(screen.scroll_top)
                .min(screen.scroll_bottom);
        }
        screen.cursor_col = col.min(screen.cols.saturating_sub(1));
        screen.wrap_pending = false;
    }

    pub(crate) fn set_cursor_row(&mut self, row: usize) {
        let origin_mode = self.origin_mode;
        let screen = self.active_mut();
        screen.cursor_row = if origin_mode {
            screen
                .scroll_top
                .saturating_add(row)
                .min(screen.scroll_bottom)
        } else {
            row.min(screen.rows.saturating_sub(1))
        };
        screen.wrap_pending = false;
    }

    pub(crate) fn set_cursor_position(&mut self, row: usize, col: usize) {
        let origin_mode = self.origin_mode;
        let screen = self.active_mut();
        screen.cursor_row = if origin_mode {
            screen
                .scroll_top
                .saturating_add(row)
                .min(screen.scroll_bottom)
        } else {
            row.min(screen.rows.saturating_sub(1))
        };
        screen.cursor_col = col.min(screen.cols.saturating_sub(1));
        screen.wrap_pending = false;
    }

    pub(crate) fn save_cursor(&mut self) {
        self.active_mut().save_cursor();
    }

    pub(crate) fn restore_cursor(&mut self) {
        self.active_mut().restore_cursor();
    }

    pub(crate) fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let rows = self.rows();
        // Validate the requested margins before clipping them to the grid. VTE
        // accepts a valid range whose bottom lies below the viewport, then
        // clamps it; that can intentionally produce a one-line region.
        if top >= bottom || top >= rows {
            return;
        }
        let bottom = bottom.min(rows.saturating_sub(1));
        let origin_mode = self.origin_mode;
        self.primary.scroll_top = top;
        self.primary.scroll_bottom = bottom;
        if let Some(alternate) = &mut self.alternate {
            alternate.scroll_top = top;
            alternate.scroll_bottom = bottom;
        }
        let screen = self.active_mut();
        let row = if origin_mode { top } else { 0 };
        screen.cursor_row = row;
        screen.cursor_col = 0;
        screen.wrap_pending = false;
    }

    pub(crate) fn reset_scroll_region(&mut self) {
        let origin_mode = self.origin_mode;
        let primary_bottom = self.primary.rows.saturating_sub(1);
        self.primary.scroll_top = 0;
        self.primary.scroll_bottom = primary_bottom;
        if let Some(alternate) = &mut self.alternate {
            alternate.scroll_top = 0;
            alternate.scroll_bottom = alternate.rows.saturating_sub(1);
        }
        let screen = self.active_mut();
        screen.cursor_row = if origin_mode { screen.scroll_top } else { 0 };
        screen.cursor_col = 0;
        screen.wrap_pending = false;
    }

    pub(crate) fn erase_display(&mut self, mode: u16, selective: bool) {
        let (row, col, rows, cols) = {
            let screen = self.active();
            (
                screen.cursor_row,
                screen.cursor_col,
                screen.rows,
                screen.cols,
            )
        };
        let clears_empty_rows = self.pen().blank() != DEFAULT_CELL;
        match mode {
            0 => {
                self.erase_row_range(row, col, cols.saturating_sub(1), selective);
                for target in row + 1..rows {
                    if clears_empty_rows || self.active().row_extent(target) > 0 {
                        self.erase_row_range(target, 0, cols.saturating_sub(1), selective);
                    }
                }
            }
            1 => {
                if row > 1 {
                    for target in 0..row {
                        if clears_empty_rows || self.active().row_extent(target) > 0 {
                            self.erase_row_range(target, 0, cols.saturating_sub(1), selective);
                        }
                    }
                }
                self.erase_row_range(row, 0, col, selective);
            }
            2 if selective => {
                for target in 0..rows {
                    self.erase_row_range(target, 0, cols.saturating_sub(1), true);
                }
            }
            2 => {
                self.effects.push_back(GridEffect::ClearViewport {
                    alternate: self.alternate_active,
                    history_size: self.history_size(),
                    rows,
                    cols,
                });
                let blank = self.pen().blank();
                if self.alternate_active {
                    self.active_mut().fill(blank);
                } else {
                    self.resize_anchor_suppressed_after_clear = true;
                    let positions = (0..rows)
                        .rev()
                        .find(|&target| {
                            self.primary
                                .row(target)
                                .iter()
                                .any(|cell| !cell_is_empty_for_viewport(cell))
                        })
                        .map_or_else(|| usize::from(self.history.is_empty()), |target| target + 1);
                    let wrap_pending = self.primary.wrap_pending;
                    if positions > 0 {
                        self.shift_rows_up(0, rows - 1, positions, true);
                    }
                    let reset_rows = rows.saturating_sub(positions);
                    for target in 0..reset_rows {
                        self.primary.cells[target].fill(blank);
                        self.primary.row_extents[target] =
                            if blank == DEFAULT_CELL { 0 } else { cols };
                    }
                    self.primary.wrap_pending = wrap_pending;
                }
                self.damage.mark_full();
            }
            3 if !selective => {
                self.clear_history();
            }
            _ => {}
        }
    }

    pub(crate) fn erase_line(&mut self, mode: u16, selective: bool) {
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };
        match mode {
            0 if self.active().wrap_pending => {}
            0 => self.erase_row_range(row, col, cols.saturating_sub(1), selective),
            1 => self.erase_row_range(row, 0, col, selective),
            2 => self.erase_row_range(row, 0, cols.saturating_sub(1), selective),
            _ => {}
        }
    }

    pub(crate) fn erase_chars(&mut self, count: usize) {
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };
        self.erase_row_range(row, col, col.saturating_add(count - 1).min(cols - 1), false);
    }

    pub(super) fn erase_row_range(
        &mut self,
        row: usize,
        mut left: usize,
        mut right: usize,
        selective: bool,
    ) {
        if left > right || row >= self.rows() {
            return;
        }
        let cols = self.cols();
        if selective {
            let cells = self.active().row(row);
            if cells[left].wide_spacer() {
                left = left.saturating_sub(1);
            }
            if right + 1 < cols && cells[right + 1].wide_spacer() {
                right += 1;
            }
        }
        let blank = self.pen().blank();
        let old_extent = self.active().row_extent(row);
        if !selective && blank == DEFAULT_CELL && (old_extent == 0 || left >= old_extent) {
            return;
        }
        let fill_right = if !selective && blank == DEFAULT_CELL {
            right.min(old_extent - 1)
        } else {
            right
        };
        {
            let screen = self.active_mut();
            let row_cells = &mut screen.cells[row];
            if selective {
                let mut col = left;
                while col <= right {
                    let pair_start = if row_cells[col].wide_spacer() {
                        col.saturating_sub(1)
                    } else {
                        col
                    };
                    let pair_end =
                        if pair_start + 1 < cols && row_cells[pair_start + 1].wide_spacer() {
                            pair_start + 1
                        } else {
                            pair_start
                        };
                    if !row_cells[pair_start..=pair_end].iter().any(Cell::protected) {
                        row_cells[pair_start..=pair_end].fill(blank);
                    }
                    col = pair_end.saturating_add(1);
                }
            } else {
                row_cells[left..=fill_right].fill(blank);
            }

            screen.row_extents[row] = if blank != DEFAULT_CELL {
                old_extent.max(right.saturating_add(1))
            } else if selective {
                HistoryRow::stored_len_for(row_cells, cols)
            } else if right.saturating_add(1) >= old_extent {
                old_extent.min(left)
            } else {
                old_extent
            };
        }
        self.damage.mark(row, left, right);
    }

    pub(crate) fn insert_blank_chars(&mut self, count: usize) {
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };
        let count = count.min(cols.saturating_sub(col));
        if count == 0 {
            return;
        }
        let blank = self.pen().blank();
        let row_cells = self.active_mut().row_mut(row);
        row_cells.copy_within(col..cols - count, col + count);
        row_cells[col..col + count].fill(blank);
        self.damage.mark(row, col, cols - 1);
    }

    pub(crate) fn delete_chars(&mut self, count: usize) {
        let (row, col, cols) = {
            let screen = self.active();
            (screen.cursor_row, screen.cursor_col, screen.cols)
        };
        let count = count.min(cols);
        if count == 0 {
            return;
        }
        let end = col.saturating_add(count).min(cols - 1);
        let num_cells = cols - end;
        let blank = self.pen().blank();
        let row_cells = self.active_mut().row_mut(row);
        for offset in 0..num_cells {
            row_cells.swap(col + offset, end + offset);
        }
        row_cells[cols - count..].fill(blank);
        self.damage.mark(row, col, cols - 1);
    }

    pub(crate) fn insert_lines(&mut self, count: usize) {
        let (row, top, bottom) = {
            let screen = self.active();
            (screen.cursor_row, screen.scroll_top, screen.scroll_bottom)
        };
        if row < top || row > bottom {
            return;
        }
        self.shift_rows_down(row, bottom, count);
    }

    pub(crate) fn delete_lines(&mut self, count: usize) {
        let (row, top, bottom) = {
            let screen = self.active();
            (screen.cursor_row, screen.scroll_top, screen.scroll_bottom)
        };
        if row < top || row > bottom {
            return;
        }
        let record_history = !self.alternate_active && row == 0;
        self.shift_rows_up(row, bottom, count, record_history);
    }

    pub(crate) fn scroll_up(&mut self, count: usize) {
        let (top, bottom) = {
            let screen = self.active();
            (screen.scroll_top, screen.scroll_bottom)
        };
        let record_history = !self.alternate_active && top == 0;
        self.shift_rows_up(top, bottom, count, record_history);
    }

    pub(crate) fn scroll_down(&mut self, count: usize) {
        let (top, bottom) = {
            let screen = self.active();
            (screen.scroll_top, screen.scroll_bottom)
        };
        self.shift_rows_down(top, bottom, count);
    }

    pub(super) fn shift_rows_up(
        &mut self,
        top: usize,
        bottom: usize,
        count: usize,
        record_history: bool,
    ) {
        let count = count.min(bottom.saturating_sub(top).saturating_add(1));
        if count == 0 {
            return;
        }
        let full_screen_region = top == 0 && bottom.saturating_add(1) == self.rows();
        let cols = self.cols();
        let alternate = self.alternate_active;
        let history_before = self.history.len();
        let blank = self.pen().blank();
        {
            let screen = self.active_mut();
            screen.cells[top..=bottom].rotate_left(count);
            screen.row_extents[top..=bottom].rotate_left(count);
        }
        for target in bottom + 1 - count..=bottom {
            let (removed, extent) = {
                let screen = self.active_mut();
                (
                    std::mem::take(&mut screen.cells[target]),
                    screen.row_extent(target),
                )
            };
            let (mut recycled, clear_len) = if record_history {
                self.push_history(removed, extent)
            } else {
                (removed, extent)
            };
            recycled.resize(cols, blank);
            if blank == DEFAULT_CELL && clear_len < cols {
                recycled[..clear_len].fill(blank);
            } else {
                recycled.fill(blank);
            }
            let screen = self.active_mut();
            screen.cells[target] = recycled;
            screen.row_extents[target] = if blank == DEFAULT_CELL { 0 } else { cols };
        }
        let history_after = self.history.len();
        self.effects.push_back(GridEffect::ScrollUp {
            alternate,
            top,
            bottom,
            count,
            full_screen_region,
            recorded_history: record_history,
            history_before,
            history_after,
        });
        if self.display_offset() == 0 {
            self.damage.scroll_up(top, bottom, count, cols);
        } else {
            self.damage.mark_full();
        }
    }

    pub(super) fn shift_rows_down(&mut self, top: usize, bottom: usize, count: usize) {
        let count = count.min(bottom.saturating_sub(top).saturating_add(1));
        if count == 0 {
            return;
        }
        let cols = self.cols();
        let alternate = self.alternate_active;
        let history_size = self.history.len();
        let blank = self.pen().blank();
        {
            let screen = self.active_mut();
            screen.cells[top..=bottom].rotate_right(count);
            screen.row_extents[top..=bottom].rotate_right(count);
            for target in top..top + count {
                let clear_len = if blank == DEFAULT_CELL {
                    screen.row_extents[target]
                } else {
                    cols
                };
                screen.cells[target][..clear_len].fill(blank);
                screen.row_extents[target] = if blank == DEFAULT_CELL { 0 } else { cols };
            }
        }
        self.effects.push_back(GridEffect::ScrollDown {
            alternate,
            top,
            bottom,
            count,
            history_size,
        });
        if self.display_offset() == 0 {
            self.damage.scroll_down(top, bottom, count, cols);
        } else {
            self.damage.mark_full();
        }
    }

    pub(super) fn push_history(&mut self, mut row: Vec<Cell>, extent: usize) -> (Vec<Cell>, usize) {
        let extent = extent.min(self.primary.cols);
        if self.history_limit == 0 {
            return (row, extent);
        }
        row.resize(self.primary.cols, DEFAULT_CELL);
        row.truncate(self.primary.cols);
        let stored_len = HistoryRow::stored_len_for(&row, extent);
        debug_assert_eq!(
            stored_len,
            HistoryRow::stored_len_for(&row, self.primary.cols),
            "live row extent must include every non-default cell"
        );

        let mut compact_storage = None;
        while self.history.len() >= self.history_limit {
            compact_storage = Some(
                self.pop_history_front()
                    .expect("a full history contains a row to evict")
                    .into_storage(),
            );
        }
        self.history.push_back(HistoryRow::copy_from_dense(
            &row,
            self.primary.cols,
            stored_len,
            compact_storage,
        ));
        if self.display_offset > 0 {
            self.display_offset = self.display_offset.saturating_add(1);
        }
        self.display_offset = self.display_offset.min(self.history.len());
        (row, stored_len)
    }

    pub(super) fn pop_history_front(&mut self) -> Option<HistoryRow> {
        let row = self.history.pop_front()?;
        self.remove_history_row_extras(&row);
        Some(row)
    }

    pub(super) fn remove_history_row_extras(&mut self, row: &HistoryRow) {
        if !row.has_pooled_extras() {
            return;
        }
        for id in row
            .iter()
            .filter_map(|cell| cell.metadata_id.filter(|id| is_pooled_extra_id(*id)))
        {
            self.extras.remove(&id);
        }
    }

    pub(super) fn release_unused_metadata(&mut self) {
        self.prune_extras();
        self.prune_hyperlinks();
        self.extras.shrink_to_fit();
        self.hyperlinks.shrink_to_fit();
        self.hyperlink_identities.shrink_to_fit();
        self.next_hyperlink_prune_len = self
            .hyperlinks
            .len()
            .saturating_add(HYPERLINK_PRUNE_INTERVAL)
            .max(HYPERLINK_PRUNE_MIN_LEN);
    }
}
