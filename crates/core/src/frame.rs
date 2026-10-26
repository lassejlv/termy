use compact_str::CompactString;

use crate::runtime::{TerminalCursorState, TerminalDamageSnapshot};
pub use crate::terminal_types::TerminalColor;

#[repr(C)]
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalRenderColor {
    #[default]
    DefaultForeground,
    DefaultBackground,
    Cursor,
    Indexed(u8),
    DimIndexed(u8),
    BrightForeground,
    DimForeground,
    Rgb(TerminalColor),
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalUnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalRenderText(CompactString);

impl TerminalRenderText {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub(crate) fn new(text: &str) -> Self {
        Self(CompactString::from(text))
    }

    pub(crate) fn from_cell_suffix(base: char, combining: Option<&str>) -> Self {
        let mut text = CompactString::default();
        text.push(base);
        if let Some(combining) = combining {
            text.push_str(combining);
        }
        Self(text)
    }

    #[cfg(test)]
    pub(crate) fn is_heap_allocated(&self) -> bool {
        self.0.is_heap_allocated()
    }
}

impl std::ops::Deref for TerminalRenderText {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl PartialEq<str> for TerminalRenderText {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for TerminalRenderText {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TerminalRenderCell {
    pub text: TerminalRenderText,
    pub foreground: TerminalRenderColor,
    pub background: TerminalRenderColor,
    pub underline_color: Option<TerminalRenderColor>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline_style: TerminalUnderlineStyle,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
    pub hyperlink: bool,
    pub wide_character_spacer: bool,
    pub leading_wide_character_spacer: bool,
    pub line_wrapped: bool,
}

impl Default for TerminalRenderCell {
    fn default() -> Self {
        Self {
            text: TerminalRenderText::default(),
            foreground: TerminalRenderColor::DefaultForeground,
            background: TerminalRenderColor::DefaultBackground,
            underline_color: None,
            bold: false,
            dim: false,
            italic: false,
            underline_style: TerminalUnderlineStyle::None,
            inverse: false,
            hidden: false,
            strikethrough: false,
            hyperlink: false,
            wide_character_spacer: false,
            leading_wide_character_spacer: false,
            line_wrapped: false,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalViewportScrollDirection {
    Up,
    Down,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalViewportScroll {
    pub top: usize,
    pub bottom: usize,
    pub count: usize,
    pub direction: TerminalViewportScrollDirection,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TerminalRenderDamageSnapshot {
    pub damage: TerminalDamageSnapshot,
    pub scrolls: Vec<TerminalViewportScroll>,
    pub generation: u64,
    pub palette_revision: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalViewportMetadata {
    pub cols: u16,
    pub rows: u16,
    pub cursor: Option<TerminalCursorState>,
    pub display_offset: usize,
    pub history_size: usize,
    pub palette_revision: u64,
    pub generation: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TerminalPalette {
    #[serde(with = "crate::remote::serde_palette")]
    pub indexed: [Option<TerminalColor>; 256],
    pub foreground: Option<TerminalColor>,
    pub background: Option<TerminalColor>,
    pub cursor: Option<TerminalColor>,
    pub revision: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TerminalRenderRead {
    pub metadata: TerminalViewportMetadata,
    pub palette: TerminalPalette,
    pub cells: Vec<TerminalRenderCell>,
    pub update: TerminalRenderDamageSnapshot,
}

/// One viewport cell. Carries no position: full frames are row-major
/// (`index = row * cols + col`) and partial updates list cells in dirty-span
/// order, so position is derived from context on the consuming side.
///
/// The layout matches the C ABI's `TermyFfiCell` field for field, so frame
/// updates reach C hosts without another pass over their cells.
#[repr(C)]
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermyCell {
    pub char: char,
    pub fg: TermyColor,
    pub bg: TermyColor,
    pub uses_terminal_default_bg: bool,
    pub bold: bool,
    pub render_text: bool,
    pub wide_character_spacer: bool,
    pub line_wrapped: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// One spare cell buffer: a host that frees its frame updates through the
/// C ABI hands theirs back here, and the next update reuses it. A host
/// polling frames then keeps one allocation instead of allocating and
/// freeing one per frame, whose pages an allocator may return to the
/// system in between.
static SPARE_CELLS: std::sync::Mutex<Vec<TermyCell>> = std::sync::Mutex::new(Vec::new());
/// Larger buffers (over 1.25 MiB) are freed rather than kept.
const MAX_SPARE_CELLS: usize = 1 << 16;

/// An empty cell buffer with room for `capacity` cells.
pub(crate) fn frame_cells(capacity: usize) -> Vec<TermyCell> {
    let mut cells = std::mem::take(
        &mut *SPARE_CELLS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    cells.reserve_exact(capacity);
    cells
}

/// Keep `cells`' allocation for the next `frame_cells`.
pub(crate) fn recycle_frame_cells(mut cells: Vec<TermyCell>) {
    if cells.capacity() > MAX_SPARE_CELLS {
        return;
    }
    cells.clear();
    let mut spare = SPARE_CELLS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if spare.capacity() < cells.capacity() {
        *spare = cells;
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TermyFrame {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<TermyCell>,
    pub cursor: Option<TerminalCursorState>,
    pub display_offset: usize,
    pub history_size: usize,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TermyFrameUpdate {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<TermyCell>,
    pub cursor: Option<TerminalCursorState>,
    pub display_offset: usize,
    pub history_size: usize,
    pub damage: TerminalDamageSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Terminal, TerminalDirtySpan, TerminalQueryColors, TerminalSize};

    // A returned buffer is handed out again, empty; an oversized one isn't
    // kept. The spare is shared, so other tests may take it in between:
    // only what a test can observe on its own is asserted.
    #[test]
    fn frame_cell_buffers_are_reused_empty() {
        let mut cells = frame_cells(100);
        assert!(cells.is_empty() && cells.capacity() >= 100);
        cells.push(TermyCell {
            char: 'x',
            fg: TermyColor::default(),
            bg: TermyColor::default(),
            uses_terminal_default_bg: false,
            bold: false,
            render_text: true,
            wide_character_spacer: false,
            line_wrapped: false,
            italic: false,
            underline: false,
            strikethrough: false,
        });
        recycle_frame_cells(cells);
        let cells = frame_cells(10);
        assert!(cells.is_empty() && cells.capacity() >= 10);
        recycle_frame_cells(Vec::with_capacity(MAX_SPARE_CELLS + 1));
        assert!(frame_cells(0).capacity() <= MAX_SPARE_CELLS);
    }

    #[test]
    fn default_render_cell_uses_role_correct_terminal_colors() {
        let cell = TerminalRenderCell::default();
        assert_eq!(cell.foreground, TerminalRenderColor::DefaultForeground);
        assert_eq!(cell.background, TerminalRenderColor::DefaultBackground);
    }

    #[test]
    fn snapshot_contains_visible_output() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let term = Terminal::new_display(size, None);
        term.feed_output(b"ok");

        let frame = term.snapshot();

        assert_eq!(frame.cols, 4);
        assert_eq!(frame.rows, 2);
        assert_eq!(frame.cells[0].char, 'o');
        assert_eq!(frame.cells[1].char, 'k');
        assert_eq!(frame.cells.len(), 8);
    }

    #[test]
    fn legacy_snapshot_keeps_single_codepoint_and_wide_spacer_semantics() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let term = Terminal::new_display(size, None);
        term.feed_output("e\u{301}界".as_bytes());

        let frame = term.snapshot();

        assert_eq!(frame.cells.len(), 8);
        assert_eq!(frame.cells[0].char, 'e');
        assert!(frame.cells[0].render_text);
        assert_eq!(frame.cells[1].char, '界');
        assert!(frame.cells[1].render_text);
        assert!(frame.cells[2].wide_character_spacer);
        assert!(!frame.cells[2].render_text);
    }

    #[test]
    fn snapshot_marks_the_cell_that_soft_wraps_a_row() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let term = Terminal::new_display(size, None);
        term.feed_output(b"abcde");

        let frame = term.snapshot();

        assert!(frame.cells[3].line_wrapped);
        assert!(!frame.cells[4].line_wrapped);
    }

    #[test]
    fn snapshot_brightens_bold_named_foreground_colors() {
        let size = TerminalSize {
            cols: 2,
            rows: 1,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let term = Terminal::new_display(size, None);
        term.feed_output(b"\x1b[31;1mX");

        let frame = term.snapshot();

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
    fn snapshot_preserves_sgr_text_attributes() {
        let size = TerminalSize {
            cols: 3,
            rows: 1,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let term = Terminal::new_display(size, None);
        term.feed_output(b"\x1b[3mI\x1b[0m\x1b[4mU\x1b[0m\x1b[9mS\x1b[0m");

        let frame = term.snapshot();

        assert!(frame.cells[0].italic);
        assert!(!frame.cells[0].underline);
        assert!(!frame.cells[0].strikethrough);
        assert!(!frame.cells[1].italic);
        assert!(frame.cells[1].underline);
        assert!(!frame.cells[1].strikethrough);
        assert!(!frame.cells[2].italic);
        assert!(!frame.cells[2].underline);
        assert!(frame.cells[2].strikethrough);
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
        let term = Terminal::new_display(size, None);
        term.feed_output(b"\x1b[7mX");

        let frame = term.snapshot();

        assert!(!frame.cells[0].uses_terminal_default_bg);
        // Inverse swaps fg/bg, so the cell background is the default foreground.
        let color = TerminalQueryColors::default().foreground;
        let default_fg = TermyColor {
            r: color.r,
            g: color.g,
            b: color.b,
            a: 255,
        };
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
        let term = Terminal::new_display(size, None);
        term.feed_output(b"\x1b[44mX");

        let frame = term.snapshot();

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
        let term = Terminal::new_display(size, None);
        term.feed_output(b"ok");

        let update = term.frame_update(true);

        assert!(matches!(update.damage, TerminalDamageSnapshot::Full));
        assert_eq!(update.cells.len(), 8);
        assert_eq!(update.cells[0].char, 'o');
        assert_eq!(update.cells[1].char, 'k');
    }

    fn range_chars(term: &Terminal, spans: &[TerminalDirtySpan]) -> Vec<char> {
        let generation = term.render_read(false).metadata.generation;
        let mut chars = Vec::new();
        assert!(
            term.visit_viewport_ranges_at_generation(generation, spans, |_, _, _, _, cell| chars
                .push(cell.text.chars().next().unwrap_or(' ')))
        );
        chars
    }

    fn display(cols: u16, rows: u16, input: &[u8]) -> Terminal {
        let terminal = Terminal::new_display(
            TerminalSize {
                cols,
                rows,
                cell_width: 9.0,
                cell_height: 18.0,
            },
            None,
        );
        terminal.feed_output(input);
        terminal
    }

    #[test]
    fn partial_reads_return_only_dirty_span_cells() {
        let term = display(4, 2, b"abcd");
        assert_eq!(
            range_chars(
                &term,
                &[TerminalDirtySpan {
                    row: 0,
                    left_col: 1,
                    right_col: 2
                }]
            ),
            vec!['b', 'c']
        );
    }

    #[test]
    fn partial_reads_include_default_cells_for_dirty_blanks() {
        let term = display(4, 2, b"x");
        assert_eq!(
            range_chars(
                &term,
                &[TerminalDirtySpan {
                    row: 0,
                    left_col: 1,
                    right_col: 3
                }]
            ),
            vec![' ', ' ', ' ']
        );
    }

    #[test]
    fn partial_reads_clip_dirty_ranges_to_the_viewport() {
        let term = display(5, 3, b"abcde\r\nfghij\r\nklmno");
        let spans = [
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
        ];
        assert_eq!(
            range_chars(&term, &spans),
            vec!['b', 'c', 'd', 'h', 'i', 'j']
        );
    }

    #[test]
    fn partial_frame_update_replays_to_a_full_snapshot() {
        let term = display(8, 3, b"first\r\nsecond");
        let mut cached = term.frame_update(true).cells;
        term.feed_output(b"\x1b[1;3HX\x1b[2;2H\x1b[2X");
        let update = term.frame_update(false);
        let TerminalDamageSnapshot::Partial(spans) = update.damage else {
            panic!("cell edits should retain partial damage");
        };
        let mut index = 0;
        for span in spans {
            for col in span.left_col..=span.right_col {
                cached[span.row * 8 + col] = update.cells[index];
                index += 1;
            }
        }
        assert_eq!(index, update.cells.len());
        assert_eq!(cached, term.snapshot().cells);
    }
}
