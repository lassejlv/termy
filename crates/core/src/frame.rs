use alacritty_terminal::{
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line},
    term::{
        Term,
        cell::{Cell, Flags},
        color::Colors,
    },
    vte::ansi::{Color as AnsiColor, NamedColor, Rgb as AnsiRgb},
};

use crate::{
    protocol::TerminalQueryColors,
    runtime::{TerminalCursorState, TerminalDamageSnapshot, TerminalSize, cursor_state_from_term},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// One viewport cell. Carries no position: full frames are row-major
/// (`index = row * cols + col`) and partial updates list cells in dirty-span
/// order, so position is derived from context on the consuming side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermyCell {
    pub char: char,
    pub fg: TermyColor,
    pub bg: TermyColor,
    pub uses_terminal_default_bg: bool,
    pub bold: bool,
    pub italic: bool,
    /// Set for any underline style. The distinction between single, double,
    /// curly, dotted and dashed is not carried: renderers that only draw a
    /// single rule would have to collapse it anyway.
    pub underline: bool,
    pub strikethrough: bool,
    pub render_text: bool,
    pub wide_character_spacer: bool,
    pub line_wrapped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermyFrame {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<TermyCell>,
    pub cursor: Option<TerminalCursorState>,
    pub display_offset: usize,
    pub history_size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermyFrameUpdate {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<TermyCell>,
    pub cursor: Option<TerminalCursorState>,
    pub display_offset: usize,
    pub history_size: usize,
    pub damage: TerminalDamageSnapshot,
}

fn rgba(rgb: AnsiRgb) -> TermyColor {
    TermyColor {
        r: rgb.r,
        g: rgb.g,
        b: rgb.b,
        a: 255,
    }
}

fn color_to_rgba(
    color: AnsiColor,
    live_colors: &Colors,
    query_colors: TerminalQueryColors,
) -> TermyColor {
    match color {
        AnsiColor::Spec(rgb) => rgba(rgb),
        AnsiColor::Indexed(index) => query_colors
            .resolve_color(live_colors, usize::from(index))
            .map(rgba)
            .unwrap_or_default(),
        AnsiColor::Named(name) => query_colors
            .resolve_color(live_colors, name as usize)
            .map_or_else(|| rgba(query_colors.foreground), rgba),
    }
}

fn bold_foreground_color(color: AnsiColor) -> AnsiColor {
    match color {
        AnsiColor::Named(NamedColor::Black) => AnsiColor::Named(NamedColor::BrightBlack),
        AnsiColor::Named(NamedColor::Red) => AnsiColor::Named(NamedColor::BrightRed),
        AnsiColor::Named(NamedColor::Green) => AnsiColor::Named(NamedColor::BrightGreen),
        AnsiColor::Named(NamedColor::Yellow) => AnsiColor::Named(NamedColor::BrightYellow),
        AnsiColor::Named(NamedColor::Blue) => AnsiColor::Named(NamedColor::BrightBlue),
        AnsiColor::Named(NamedColor::Magenta) => AnsiColor::Named(NamedColor::BrightMagenta),
        AnsiColor::Named(NamedColor::Cyan) => AnsiColor::Named(NamedColor::BrightCyan),
        AnsiColor::Named(NamedColor::White) => AnsiColor::Named(NamedColor::BrightWhite),
        _ => color,
    }
}

fn cell_from_renderable_cell(
    cell: &Cell,
    live_colors: &Colors,
    query_colors: TerminalQueryColors,
) -> TermyCell {
    let mut fg = cell.fg;
    let mut bg = cell.bg;
    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    // Compute default-bg *after* the inverse swap: a reverse-video cell
    // paints with the default foreground color, so it is no longer the
    // terminal's default background and must be drawn.
    let uses_terminal_default_bg = matches!(bg, AnsiColor::Named(NamedColor::Background));
    if cell.flags.contains(Flags::BOLD) {
        fg = bold_foreground_color(fg);
    }

    let mut fg = color_to_rgba(fg, live_colors, query_colors);
    if cell.flags.contains(Flags::DIM) {
        fg.r /= 2;
        fg.g /= 2;
        fg.b /= 2;
    }

    let wide_character_spacer = cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);

    TermyCell {
        char: cell.c,
        fg,
        bg: color_to_rgba(bg, live_colors, query_colors),
        uses_terminal_default_bg,
        bold: cell.flags.contains(Flags::BOLD),
        italic: cell.flags.contains(Flags::ITALIC),
        underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
        strikethrough: cell.flags.contains(Flags::STRIKEOUT),
        render_text: !wide_character_spacer
            && !cell.flags.contains(Flags::HIDDEN)
            && cell.c != '\0'
            && !cell.c.is_control(),
        wide_character_spacer,
        line_wrapped: cell.flags.contains(Flags::WRAPLINE),
    }
}

pub(crate) fn snapshot_from_term<T: EventListener>(
    term: &Term<T>,
    size: TerminalSize,
    query_colors: TerminalQueryColors,
) -> TermyFrame {
    let cols = usize::from(size.cols);
    let rows = usize::from(size.rows);
    let live_colors = term.colors();
    let content = term.renderable_content();
    // Alacritty's display iterator is exact-size and row-major over the
    // visible viewport. Build the final cell buffer in one pass instead of
    // template-filling the entire grid and overwriting every entry.
    let mut cells = Vec::with_capacity(cols.saturating_mul(rows));
    for indexed_cell in content.display_iter {
        cells.push(cell_from_renderable_cell(
            indexed_cell.cell,
            live_colors,
            query_colors,
        ));
    }
    debug_assert_eq!(cells.len(), cols.saturating_mul(rows));

    let grid = term.grid();
    TermyFrame {
        cols: size.cols,
        rows: size.rows,
        cells,
        cursor: cursor_state_from_term(term),
        display_offset: grid.display_offset(),
        history_size: grid.history_size(),
    }
}

pub(crate) fn snapshot_update_from_term<T: EventListener>(
    term: &Term<T>,
    size: TerminalSize,
    query_colors: TerminalQueryColors,
    damage: TerminalDamageSnapshot,
) -> TermyFrameUpdate {
    let cols = usize::from(size.cols);
    let rows = usize::from(size.rows);
    let live_colors = term.colors();
    let grid = term.grid();

    let (cells, damage) = match damage {
        TerminalDamageSnapshot::Full => (
            snapshot_from_term(term, size, query_colors).cells,
            TerminalDamageSnapshot::Full,
        ),
        TerminalDamageSnapshot::Partial(mut spans) => {
            spans.retain_mut(|span| {
                if span.row >= rows || cols == 0 {
                    return false;
                }
                let left_col = span.left_col.min(cols.saturating_sub(1));
                let right_col = span.right_col.min(cols.saturating_sub(1));
                if left_col > right_col {
                    return false;
                }
                span.left_col = left_col;
                span.right_col = right_col;
                true
            });

            let cell_count = spans.iter().fold(0usize, |count, span| {
                count.saturating_add(span.right_col - span.left_col + 1)
            });

            // Every normalized span maps to a valid grid slice. Push the final
            // cells directly instead of template-filling the output and then
            // overwriting every entry in a second pass.
            let mut cells = Vec::with_capacity(cell_count);
            let display_offset = grid.display_offset() as i32;
            for span in &spans {
                let line = Line(span.row as i32 - display_offset);
                for col in span.left_col..=span.right_col {
                    let cell = &grid[line][Column(col)];
                    cells.push(cell_from_renderable_cell(cell, live_colors, query_colors));
                }
            }

            (cells, TerminalDamageSnapshot::Partial(spans))
        }
    };

    TermyFrameUpdate {
        cols: size.cols,
        rows: size.rows,
        cells,
        cursor: cursor_state_from_term(term),
        display_offset: grid.display_offset(),
        history_size: grid.history_size(),
        damage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::TerminalDirtySpan;
    use alacritty_terminal::{event::VoidListener, term::Config as TermConfig, vte::ansi};

    #[test]
    fn snapshot_contains_visible_output() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"ok");

        let frame = snapshot_from_term(&term, size, TerminalQueryColors::default());

        assert_eq!(frame.cols, 4);
        assert_eq!(frame.rows, 2);
        assert_eq!(frame.cells[0].char, 'o');
        assert_eq!(frame.cells[1].char, 'k');
        assert_eq!(frame.cells.len(), 8);
    }

    #[test]
    fn snapshot_marks_the_cell_that_soft_wraps_a_row() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"abcde");

        let frame = snapshot_from_term(&term, size, TerminalQueryColors::default());

        assert!(frame.cells[3].line_wrapped);
        assert!(!frame.cells[4].line_wrapped);
    }

    #[test]
    fn snapshot_carries_text_attributes() {
        let size = TerminalSize {
            cols: 4,
            rows: 1,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        // Italic, then underline, then strikethrough, then a plain cell.
        parser.advance(&mut term, b"\x1b[3mi\x1b[0m\x1b[4mu\x1b[0m\x1b[9ms\x1b[0mp");

        let frame = snapshot_from_term(&term, size, TerminalQueryColors::default());

        assert!(frame.cells[0].italic);
        assert!(frame.cells[1].underline);
        assert!(frame.cells[2].strikethrough);

        let plain = frame.cells[3];
        assert!(!plain.italic);
        assert!(!plain.underline);
        assert!(!plain.strikethrough);
    }

    #[test]
    fn snapshot_reports_every_underline_style_as_underlined() {
        let size = TerminalSize {
            cols: 2,
            rows: 1,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        // Double and curly underline both collapse to `underline`. Written as
        // SGR 4 subparameters: bare `21` is read as bold-off by this parser,
        // not as a double underline.
        parser.advance(&mut term, b"\x1b[4:2md\x1b[0m\x1b[4:3mc");

        let frame = snapshot_from_term(&term, size, TerminalQueryColors::default());

        assert!(frame.cells[0].underline);
        assert!(frame.cells[1].underline);
    }

    #[test]
    fn snapshot_brightens_bold_named_foreground_colors() {
        let size = TerminalSize {
            cols: 2,
            rows: 1,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"\x1b[31;1mX");

        let frame = snapshot_from_term(&term, size, TerminalQueryColors::default());

        assert_eq!(
            frame.cells[0].fg,
            TermyColor {
                r: 0xff,
                g: 0x00,
                b: 0x00,
                a: 255,
            }
        );
        assert!(frame.cells[0].bold);
    }

    #[test]
    fn snapshot_inverse_default_cell_paints_background() {
        // Ink/Claude Code render the cursor as a reverse-video cell with the
        // terminal's default colors. After the inverse swap its background is
        // the default foreground, so it must NOT be flagged as default-bg or
        // the renderer skips it and the cursor disappears.
        let size = TerminalSize {
            cols: 2,
            rows: 1,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"\x1b[7mX");

        let frame = snapshot_from_term(&term, size, TerminalQueryColors::default());

        assert!(!frame.cells[0].uses_terminal_default_bg);
        // Inverse swaps fg/bg, so the cell background is the default foreground.
        let default_fg = color_to_rgba(
            AnsiColor::Named(NamedColor::Foreground),
            term.colors(),
            TerminalQueryColors::default(),
        );
        assert_eq!(frame.cells[0].bg, default_fg);
    }

    #[test]
    fn snapshot_marks_explicit_backgrounds() {
        let size = TerminalSize {
            cols: 2,
            rows: 1,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"\x1b[44mX");

        let frame = snapshot_from_term(&term, size, TerminalQueryColors::default());

        assert!(!frame.cells[0].uses_terminal_default_bg);
        assert_eq!(
            frame.cells[0].bg,
            TermyColor {
                r: 0x00,
                g: 0x00,
                b: 0xee,
                a: 255,
            }
        );
    }

    #[test]
    fn snapshot_update_full_returns_all_visible_cells() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"ok");

        let update = snapshot_update_from_term(
            &term,
            size,
            TerminalQueryColors::default(),
            TerminalDamageSnapshot::Full,
        );

        assert!(matches!(update.damage, TerminalDamageSnapshot::Full));
        assert_eq!(update.cells.len(), 8);
        assert_eq!(update.cells[0].char, 'o');
        assert_eq!(update.cells[1].char, 'k');
    }

    #[test]
    fn snapshot_update_partial_returns_only_dirty_span_cells() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"abcd");

        let update = snapshot_update_from_term(
            &term,
            size,
            TerminalQueryColors::default(),
            TerminalDamageSnapshot::Partial(vec![TerminalDirtySpan {
                row: 0,
                left_col: 1,
                right_col: 2,
            }]),
        );

        assert_eq!(
            update.damage,
            TerminalDamageSnapshot::Partial(vec![TerminalDirtySpan {
                row: 0,
                left_col: 1,
                right_col: 2,
            }])
        );
        // Cells are listed in dirty-span order: (row 0, cols 1..=2).
        assert_eq!(update.cells.len(), 2);
        assert_eq!(update.cells[0].char, 'b');
        assert_eq!(update.cells[1].char, 'c');
    }

    #[test]
    fn snapshot_update_partial_includes_default_cells_for_dirty_blanks() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"x");

        let update = snapshot_update_from_term(
            &term,
            size,
            TerminalQueryColors::default(),
            TerminalDamageSnapshot::Partial(vec![TerminalDirtySpan {
                row: 0,
                left_col: 1,
                right_col: 3,
            }]),
        );

        // One cell per dirty column (1..=3), all blanks.
        assert_eq!(update.cells.len(), 3);
        assert!(update.cells.iter().all(|cell| cell.char == ' '));
    }

    #[test]
    fn snapshot_update_partial_reads_only_clipped_dirty_cells() {
        let size = TerminalSize {
            cols: 5,
            rows: 3,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let mut term = Term::new(TermConfig::default(), &size, VoidListener);
        let mut parser: ansi::Processor = ansi::Processor::new();
        parser.advance(&mut term, b"abcde\r\nfghij\r\nklmno");

        let update = snapshot_update_from_term(
            &term,
            size,
            TerminalQueryColors::default(),
            TerminalDamageSnapshot::Partial(vec![
                TerminalDirtySpan {
                    row: 0,
                    left_col: 1,
                    right_col: 3,
                },
                TerminalDirtySpan {
                    row: 1,
                    left_col: 2,
                    right_col: 99,
                },
                TerminalDirtySpan {
                    row: 99,
                    left_col: 0,
                    right_col: 4,
                },
            ]),
        );

        assert_eq!(
            update.damage,
            TerminalDamageSnapshot::Partial(vec![
                TerminalDirtySpan {
                    row: 0,
                    left_col: 1,
                    right_col: 3,
                },
                TerminalDirtySpan {
                    row: 1,
                    left_col: 2,
                    right_col: 4,
                },
            ])
        );
        // Cells follow the clipped spans in order: (0,1..=3) then (1,2..=4).
        assert_eq!(
            update
                .cells
                .iter()
                .map(|cell| cell.char)
                .collect::<Vec<_>>(),
            vec!['b', 'c', 'd', 'h', 'i', 'j']
        );
    }
}
