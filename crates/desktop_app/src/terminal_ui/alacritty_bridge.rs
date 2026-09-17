use alacritty_terminal::{
    event::VoidListener,
    grid::Dimensions,
    index::{Column, Line},
    term::{Term, TermMode, cell::Flags},
    vte::ansi::{self, Handler, NamedPrivateMode, PrivateMode},
};
use termy_core::{
    DetectedLink, DetectedViewportLink, KittyGraphicsCursorTracker, KittyGraphicsScreen,
    KittyGraphicsState, TerminalCursorState, TerminalCursorStyle, TerminalDamageSnapshot,
    TerminalDirtySpan, TerminalKeyboardMode, TerminalMouseMode, TerminalOptions, find_link_in_line,
};
use unicode_width::UnicodeWidthChar;

pub(crate) fn term_config(options: TerminalOptions) -> alacritty_terminal::term::Config {
    let shape = match options.default_cursor_style {
        TerminalCursorStyle::Line => ansi::CursorShape::Beam,
        TerminalCursorStyle::Block => ansi::CursorShape::Block,
    };
    alacritty_terminal::term::Config {
        scrolling_history: options
            .scrollback_history
            .min(termy_core::MAX_TERMINAL_SCROLLBACK_HISTORY),
        default_cursor_style: ansi::CursorStyle {
            shape,
            blinking: false,
        },
        kitty_keyboard: true,
        ..alacritty_terminal::term::Config::default()
    }
}

pub(crate) fn cursor_state(term: &Term<VoidListener>) -> Option<TerminalCursorState> {
    let cursor = term.renderable_content().cursor;
    let style = match cursor.shape {
        ansi::CursorShape::Hidden => return None,
        ansi::CursorShape::Block | ansi::CursorShape::HollowBlock => TerminalCursorStyle::Block,
        ansi::CursorShape::Underline | ansi::CursorShape::Beam => TerminalCursorStyle::Line,
    };
    Some(TerminalCursorState {
        col: cursor.point.column.0,
        row: usize::try_from(cursor.point.line.0).ok()?,
        style,
    })
}

pub(crate) fn cursor_position(term: &Term<VoidListener>) -> (usize, usize) {
    let cursor = term.renderable_content().cursor;
    (
        cursor.point.column.0,
        usize::try_from(cursor.point.line.0).ok().unwrap_or(0),
    )
}

pub(crate) fn mouse_mode(mode: TermMode) -> TerminalMouseMode {
    TerminalMouseMode {
        enabled: mode.intersects(TermMode::MOUSE_MODE) && !mode.contains(TermMode::VI),
        report_click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
        report_drag: mode.contains(TermMode::MOUSE_DRAG),
        report_motion: mode.contains(TermMode::MOUSE_MOTION),
        sgr_encoding: mode.contains(TermMode::SGR_MOUSE),
        utf8_encoding: mode.contains(TermMode::UTF8_MOUSE),
    }
}

pub(crate) fn keyboard_mode(mode: TermMode) -> TerminalKeyboardMode {
    TerminalKeyboardMode::from_flags(
        mode.contains(TermMode::APP_CURSOR),
        mode.contains(TermMode::DISAMBIGUATE_ESC_CODES),
        mode.contains(TermMode::REPORT_EVENT_TYPES),
        mode.contains(TermMode::REPORT_ALTERNATE_KEYS),
        mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
        mode.contains(TermMode::REPORT_ASSOCIATED_TEXT),
    )
}

pub(crate) fn hyperlink_at(
    term: &Term<VoidListener>,
    row: usize,
    col: usize,
) -> Option<DetectedLink> {
    let grid = term.grid();
    let columns = grid.columns();
    if row >= grid.screen_lines() || col >= columns {
        return None;
    }

    let line = Line(row as i32 - grid.display_offset() as i32);
    let row_cells = &grid[line];
    let target = row_cells[Column(col)].hyperlink()?;
    let matches_target = |candidate_col: usize| {
        row_cells[Column(candidate_col)]
            .hyperlink()
            .is_some_and(|other| other == target)
    };

    let mut start_col = col;
    while start_col > 0 && matches_target(start_col - 1) {
        start_col -= 1;
    }
    let mut end_col = col;
    while end_col + 1 < columns && matches_target(end_col + 1) {
        end_col += 1;
    }

    Some(DetectedLink {
        start_col,
        end_col,
        target: target.uri().to_string(),
    })
}

pub(crate) fn link_at(
    term: &Term<VoidListener>,
    row: usize,
    col: usize,
) -> Option<DetectedViewportLink> {
    let grid = term.grid();
    let columns = grid.columns();
    let screen_lines = grid.screen_lines();
    if row >= screen_lines || col >= columns || columns == 0 {
        return None;
    }

    let display_offset = i32::try_from(grid.display_offset()).ok()?;
    let hovered = GridPosition {
        line: i32::try_from(row).ok()?.saturating_sub(display_offset),
        col,
    };
    let bounds = grid_line_bounds(grid)?;

    if let Some(target) = grid[Line(hovered.line)][Column(hovered.col)].hyperlink() {
        let target_uri = target.uri().to_string();
        let mut start = hovered;
        while let Some(previous) = previous_wrapped_position(grid, start, bounds, columns) {
            if grid[Line(previous.line)][Column(previous.col)]
                .hyperlink()
                .is_some_and(|candidate| candidate == target)
            {
                start = previous;
            } else {
                break;
            }
        }

        let mut end = hovered;
        while let Some(next) = next_wrapped_position(grid, end, bounds, columns) {
            if grid[Line(next.line)][Column(next.col)]
                .hyperlink()
                .is_some_and(|candidate| candidate == target)
            {
                end = next;
            } else {
                break;
            }
        }

        return viewport_link_from_grid_range(
            start,
            end,
            target_uri,
            display_offset,
            screen_lines,
            columns,
        );
    }

    let hovered_char = grid_cell_char(grid, hovered);
    if hovered_char.is_whitespace() {
        return None;
    }

    let mut positions_before = Vec::new();
    let mut cursor = hovered;
    while let Some(previous) = previous_wrapped_position(grid, cursor, bounds, columns) {
        let candidate = grid_cell_char(grid, previous);
        if candidate.is_whitespace() {
            break;
        }
        positions_before.push((previous, candidate));
        cursor = previous;
    }
    positions_before.reverse();

    let mut positions = Vec::with_capacity(positions_before.len().saturating_add(16));
    positions.extend(positions_before);
    let hovered_index = positions.len();
    positions.push((hovered, hovered_char));

    cursor = hovered;
    while let Some(next) = next_wrapped_position(grid, cursor, bounds, columns) {
        let candidate = grid_cell_char(grid, next);
        if candidate.is_whitespace() {
            break;
        }
        positions.push((next, candidate));
        cursor = next;
    }

    let token: Vec<char> = positions.iter().map(|(_, character)| *character).collect();
    let detected = find_link_in_line(&token, hovered_index)?;
    let start = positions.get(detected.start_col)?.0;
    let end = positions.get(detected.end_col)?.0;
    viewport_link_from_grid_range(
        start,
        end,
        detected.target,
        display_offset,
        screen_lines,
        columns,
    )
}

#[derive(Clone, Copy)]
struct GridPosition {
    line: i32,
    col: usize,
}

fn grid_line_bounds(
    grid: &alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>,
) -> Option<(i32, i32)> {
    let screen_lines = i32::try_from(grid.screen_lines()).ok()?;
    let total_lines = i32::try_from(grid.total_lines()).ok()?;
    if screen_lines <= 0 || total_lines <= 0 {
        return None;
    }
    Some((-(total_lines - screen_lines), screen_lines - 1))
}

fn previous_wrapped_position(
    grid: &alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>,
    position: GridPosition,
    (min_line, _): (i32, i32),
    columns: usize,
) -> Option<GridPosition> {
    if position.col > 0 {
        return Some(GridPosition {
            line: position.line,
            col: position.col - 1,
        });
    }
    let previous_line = position.line.checked_sub(1)?;
    if previous_line < min_line {
        return None;
    }
    let previous_col = columns.checked_sub(1)?;
    grid[Line(previous_line)][Column(previous_col)]
        .flags
        .contains(Flags::WRAPLINE)
        .then_some(GridPosition {
            line: previous_line,
            col: previous_col,
        })
}

fn next_wrapped_position(
    grid: &alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>,
    position: GridPosition,
    (_, max_line): (i32, i32),
    columns: usize,
) -> Option<GridPosition> {
    if position.col + 1 < columns {
        return Some(GridPosition {
            line: position.line,
            col: position.col + 1,
        });
    }
    if position.line >= max_line
        || !grid[Line(position.line)][Column(position.col)]
            .flags
            .contains(Flags::WRAPLINE)
    {
        return None;
    }
    Some(GridPosition {
        line: position.line + 1,
        col: 0,
    })
}

fn grid_cell_char(
    grid: &alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>,
    position: GridPosition,
) -> char {
    let cell = &grid[Line(position.line)][Column(position.col)];
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN)
        || cell.c == '\0'
        || cell.c.is_control()
    {
        ' '
    } else {
        cell.c
    }
}

fn viewport_link_from_grid_range(
    start: GridPosition,
    end: GridPosition,
    target: String,
    display_offset: i32,
    screen_lines: usize,
    columns: usize,
) -> Option<DetectedViewportLink> {
    let viewport_min_line = -display_offset;
    let viewport_max_line = i32::try_from(screen_lines)
        .ok()?
        .checked_sub(1)?
        .saturating_sub(display_offset);
    let visible_start_line = start.line.max(viewport_min_line);
    let visible_end_line = end.line.min(viewport_max_line);
    if visible_start_line > visible_end_line {
        return None;
    }

    Some(DetectedViewportLink {
        start_row: usize::try_from(visible_start_line.saturating_add(display_offset)).ok()?,
        start_col: if start.line < visible_start_line {
            0
        } else {
            start.col
        },
        end_row: usize::try_from(visible_end_line.saturating_add(display_offset)).ok()?,
        end_col: if end.line > visible_end_line {
            columns.saturating_sub(1)
        } else {
            end.col
        },
        target,
    })
}

pub(crate) fn take_damage_snapshot(term: &mut Term<VoidListener>) -> TerminalDamageSnapshot {
    let rows = term.grid().screen_lines();
    let cols = term.grid().columns();
    let snapshot = match term.damage() {
        alacritty_terminal::term::TermDamage::Full => TerminalDamageSnapshot::Full,
        alacritty_terminal::term::TermDamage::Partial(damage_iter) => {
            let spans = damage_iter
                .filter_map(|damage| {
                    if rows == 0 || cols == 0 || damage.line >= rows {
                        return None;
                    }
                    let left_col = damage.left.saturating_sub(1).min(cols.saturating_sub(1));
                    let right_col = damage.right.saturating_add(1).min(cols.saturating_sub(1));
                    (left_col <= right_col).then_some(TerminalDirtySpan {
                        row: damage.line,
                        left_col,
                        right_col,
                    })
                })
                .collect();
            TerminalDamageSnapshot::Partial(spans)
        }
    };
    term.reset_damage();
    snapshot
}

pub(crate) fn advance_graphics_text(
    tracker: &mut KittyGraphicsCursorTracker,
    parser: &mut ansi::Processor,
    term: &mut Term<VoidListener>,
    bytes: &[u8],
    track_scrolls: bool,
    graphics: &mut KittyGraphicsState,
) -> bool {
    let mut handler = TrackingHandler {
        term,
        tracker,
        track_scrolls,
        graphics,
        graphics_changed: false,
    };
    parser.advance(&mut handler, bytes);
    handler.graphics_changed
}

pub(crate) fn advance_graphics_cursor(
    term: &mut Term<VoidListener>,
    cols: u32,
    rows: u32,
    full_screen_scroll_region: bool,
) -> usize {
    let cols = usize::try_from(cols)
        .unwrap_or(usize::MAX)
        .min(term.grid().columns());
    Handler::move_forward(term, cols);
    let rows = usize::try_from(rows)
        .unwrap_or(usize::MAX)
        .min(term.grid().screen_lines());
    if !full_screen_scroll_region {
        Handler::move_down(term, rows);
        return 0;
    }
    let history_before = term.grid().history_size();
    let mut scrolled_lines = 0usize;
    for _ in 0..rows {
        let line_before = term.grid().cursor.point.line;
        Handler::linefeed(term);
        scrolled_lines += usize::from(term.grid().cursor.point.line == line_before);
    }
    scrolled_lines.saturating_sub(term.grid().history_size().saturating_sub(history_before))
}

struct ScrollObservation {
    screen: KittyGraphicsScreen,
    full_screen_region: bool,
    physical_lines: usize,
    history_before: usize,
}

struct TrackingHandler<'a> {
    term: &'a mut Term<VoidListener>,
    tracker: &'a mut KittyGraphicsCursorTracker,
    track_scrolls: bool,
    graphics: &'a mut KittyGraphicsState,
    graphics_changed: bool,
}

impl TrackingHandler<'_> {
    fn linefeed_scroll_lines(&self) -> usize {
        let screen_lines = self.term.grid().screen_lines();
        let full_screen_region = self.tracker.region_covers_full_screen(screen_lines);
        let cursor_line = self.term.grid().cursor.point.line.0.max(0) as usize;
        usize::from(full_screen_region && cursor_line.saturating_add(1) == screen_lines)
    }

    fn input_scroll_lines(&self, c: char) -> usize {
        let Some(width) = c.width() else {
            return 0;
        };
        if width == 0 || !self.term.mode().contains(TermMode::LINE_WRAP) {
            return 0;
        }
        let cursor = &self.term.grid().cursor;
        let needs_wrap = cursor.input_needs_wrap
            || (width == 2
                && cursor.point.column.0.saturating_add(1) >= self.term.grid().columns());
        if needs_wrap {
            self.linefeed_scroll_lines()
        } else {
            0
        }
    }

    fn observe_scroll(&self, physical_lines: usize) -> Option<ScrollObservation> {
        if !self.track_scrolls || physical_lines == 0 {
            return None;
        }
        Some(ScrollObservation {
            screen: KittyGraphicsScreen::from_alternate_screen(
                self.term.mode().contains(TermMode::ALT_SCREEN),
            ),
            full_screen_region: self
                .tracker
                .region_covers_full_screen(self.term.grid().screen_lines()),
            physical_lines,
            history_before: self.term.grid().history_size(),
        })
    }

    fn finish_scroll(&mut self, observation: Option<ScrollObservation>) {
        let Some(observation) = observation else {
            return;
        };
        let history_growth = self
            .term
            .grid()
            .history_size()
            .saturating_sub(observation.history_before);
        match (observation.screen, observation.full_screen_region) {
            (KittyGraphicsScreen::Primary, true) => {
                let lines = observation.physical_lines.saturating_sub(history_growth);
                if lines > 0 {
                    self.graphics_changed |= self
                        .graphics
                        .scroll_up_without_history_on_screen(lines, KittyGraphicsScreen::Primary);
                }
            }
            (KittyGraphicsScreen::Primary, false) if history_growth > 0 => {
                self.graphics_changed |= self
                    .graphics
                    .preserve_primary_placements_across_partial_history_growth(history_growth);
            }
            (KittyGraphicsScreen::Alternate, true) => {
                self.graphics_changed |= self.graphics.scroll_up_without_history_on_screen(
                    observation.physical_lines,
                    KittyGraphicsScreen::Alternate,
                );
            }
            _ => {}
        }
    }
}

macro_rules! forward_handler_methods {
    ($(fn $name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            fn $name(&mut self $(, $arg: $ty)*) {
                Handler::$name(&mut *self.term $(, $arg)*);
            }
        )*
    };
}

impl Handler for TrackingHandler<'_> {
    fn input(&mut self, c: char) {
        let observation = self.observe_scroll(self.input_scroll_lines(c));
        Handler::input(&mut *self.term, c);
        self.finish_scroll(observation);
    }

    fn put_tab(&mut self, count: u16) {
        let physical_lines = if self.track_scrolls
            && self.term.grid().cursor.input_needs_wrap
            && self.term.mode().contains(TermMode::LINE_WRAP)
        {
            self.linefeed_scroll_lines()
        } else {
            0
        };
        let observation = self.observe_scroll(physical_lines);
        Handler::put_tab(&mut *self.term, count);
        self.finish_scroll(observation);
    }

    fn linefeed(&mut self) {
        let observation = self.observe_scroll(self.linefeed_scroll_lines());
        Handler::linefeed(&mut *self.term);
        self.finish_scroll(observation);
    }

    fn newline(&mut self) {
        let observation = self.observe_scroll(self.linefeed_scroll_lines());
        Handler::newline(&mut *self.term);
        self.finish_scroll(observation);
    }

    fn scroll_up(&mut self, lines: usize) {
        let physical_lines = if self.track_scrolls {
            lines.min(self.term.grid().screen_lines())
        } else {
            0
        };
        let observation = self.observe_scroll(physical_lines);
        Handler::scroll_up(&mut *self.term, lines);
        self.finish_scroll(observation);
    }

    fn reset_state(&mut self) {
        self.tracker.reset_scroll_region();
        self.graphics_changed |= self
            .graphics
            .clear_visible_on_screen(KittyGraphicsScreen::Primary);
        self.graphics_changed |= self
            .graphics
            .clear_visible_on_screen(KittyGraphicsScreen::Alternate);
        Handler::reset_state(&mut *self.term);
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        if mode == NamedPrivateMode::ColumnMode.into() {
            self.tracker.reset_scroll_region();
        }
        if mode == NamedPrivateMode::SwapScreenAndSetRestoreCursor.into()
            && !self.term.mode().contains(TermMode::ALT_SCREEN)
        {
            self.graphics_changed |= self
                .graphics
                .clear_visible_on_screen(KittyGraphicsScreen::Alternate);
        }
        Handler::set_private_mode(&mut *self.term, mode);
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        if mode == NamedPrivateMode::ColumnMode.into() {
            self.tracker.reset_scroll_region();
        }
        Handler::unset_private_mode(&mut *self.term, mode);
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        self.tracker
            .set_scroll_region(top, bottom, self.term.grid().screen_lines());
        Handler::set_scrolling_region(&mut *self.term, top, bottom);
    }

    fn clear_screen(&mut self, mode: ansi::ClearMode) {
        let clear_viewport = self.track_scrolls && matches!(mode, ansi::ClearMode::All);
        let screen = KittyGraphicsScreen::from_alternate_screen(
            self.term.mode().contains(TermMode::ALT_SCREEN),
        );
        let history_size = self.term.grid().history_size();
        let rows = self.term.grid().screen_lines();
        let cols = self.term.grid().columns();
        Handler::clear_screen(&mut *self.term, mode);
        if clear_viewport {
            self.graphics_changed |=
                self.graphics
                    .clear_viewport_on_screen(screen, history_size, rows, cols);
        }
    }

    forward_handler_methods! {
        fn set_title(title: Option<String>);
        fn set_cursor_style(style: Option<ansi::CursorStyle>);
        fn set_cursor_shape(shape: ansi::CursorShape);
        fn goto(line: i32, col: usize);
        fn goto_line(line: i32);
        fn goto_col(col: usize);
        fn insert_blank(count: usize);
        fn move_up(rows: usize);
        fn move_down(rows: usize);
        fn identify_terminal(intermediate: Option<char>);
        fn device_status(status: usize);
        fn move_forward(cols: usize);
        fn move_backward(cols: usize);
        fn move_down_and_cr(rows: usize);
        fn move_up_and_cr(rows: usize);
        fn backspace();
        fn carriage_return();
        fn bell();
        fn substitute();
        fn set_horizontal_tabstop();
        fn scroll_down(rows: usize);
        fn insert_blank_lines(lines: usize);
        fn delete_lines(lines: usize);
        fn erase_chars(count: usize);
        fn delete_chars(count: usize);
        fn move_backward_tabs(count: u16);
        fn move_forward_tabs(count: u16);
        fn save_cursor_position();
        fn restore_cursor_position();
        fn clear_line(mode: ansi::LineClearMode);
        fn clear_tabs(mode: ansi::TabulationClearMode);
        fn set_tabs(interval: u16);
        fn reverse_index();
        fn terminal_attribute(attr: ansi::Attr);
        fn set_mode(mode: ansi::Mode);
        fn unset_mode(mode: ansi::Mode);
        fn report_mode(mode: ansi::Mode);
        fn report_private_mode(mode: ansi::PrivateMode);
        fn set_keypad_application_mode();
        fn unset_keypad_application_mode();
        fn set_active_charset(index: ansi::CharsetIndex);
        fn configure_charset(index: ansi::CharsetIndex, charset: ansi::StandardCharset);
        fn set_color(index: usize, color: ansi::Rgb);
        fn dynamic_color_sequence(prefix: String, index: usize, terminator: &str);
        fn reset_color(index: usize);
        fn clipboard_store(clipboard: u8, data: &[u8]);
        fn clipboard_load(clipboard: u8, terminator: &str);
        fn decaln();
        fn push_title();
        fn pop_title();
        fn text_area_size_pixels();
        fn text_area_size_chars();
        fn set_hyperlink(hyperlink: Option<ansi::Hyperlink>);
        fn set_mouse_cursor_icon(icon: ansi::cursor_icon::CursorIcon);
        fn report_keyboard_mode();
        fn push_keyboard_mode(mode: ansi::KeyboardModes);
        fn pop_keyboard_modes(to_pop: u16);
        fn set_keyboard_mode(mode: ansi::KeyboardModes, behavior: ansi::KeyboardModesApplyBehavior);
        fn set_modify_other_keys(mode: ansi::ModifyOtherKeys);
        fn report_modify_other_keys();
        fn set_scp(char_path: ansi::ScpCharPath, update_mode: ansi::ScpUpdateMode);
    }
}
