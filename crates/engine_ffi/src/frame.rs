//! Viewport frames and buffer line reads for host-owned renderers.

use std::{collections::HashMap, ptr, sync::Arc};

use termy_core::terminal_engine::{
    Cell, Color, CursorShape, Damage, Engine, Hyperlink, Line, Style, UnderlineStyle,
};

use crate::{TermyEngine, TermyEngineStatus, guard, handle};

pub const CELL_NARROW: u8 = 0;
pub const CELL_WIDE: u8 = 1;
pub const CELL_SPACER_TAIL: u8 = 2;
pub const CELL_SPACER_HEAD: u8 = 3;

pub const FLAG_BOLD: u16 = 1;
pub const FLAG_FAINT: u16 = 1 << 1;
pub const FLAG_ITALIC: u16 = 1 << 2;
pub const FLAG_INVERSE: u16 = 1 << 3;
pub const FLAG_INVISIBLE: u16 = 1 << 4;
pub const FLAG_STRIKETHROUGH: u16 = 1 << 5;
pub const FLAG_BLINK: u16 = 1 << 6;
pub const FLAG_PROTECTED: u16 = 1 << 7;
pub const FLAG_STYLED: u16 = 1 << 8;
pub const FLAG_BLANK: u16 = 1 << 9;
pub const FLAG_WRAPPED: u16 = 1 << 10;

pub const DAMAGE_PARTIAL: u32 = 0;
pub const DAMAGE_FULL: u32 = 1;

/// Frame buffer capacity, in cells, kept across calls whatever their size.
const KEEP_CELLS: usize = 64 * 1024;

/// One cell. Colors are raw: `0` the terminal default, `0x01000000 | n`
/// palette entry `n`, `0x02RRGGBB` a direct color.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyEngineCell {
    pub codepoint: u32,
    pub grapheme_start: u32,
    pub grapheme_len: u16,
    pub width: u8,
    pub underline: u8,
    pub foreground: u32,
    pub background: u32,
    pub underline_color: u32,
    pub flags: u16,
    pub reserved: u16,
    pub hyperlink: u32,
}

const _: () = assert!(size_of::<TermyEngineCell>() == 32);
// Mirrored by tests/c/replica.c, so the header and Rust agree on every field.
const _: () = {
    use std::mem::offset_of;
    assert!(offset_of!(TermyEngineCell, grapheme_start) == 4);
    assert!(offset_of!(TermyEngineCell, grapheme_len) == 8);
    assert!(offset_of!(TermyEngineCell, width) == 10);
    assert!(offset_of!(TermyEngineCell, underline) == 11);
    assert!(offset_of!(TermyEngineCell, foreground) == 12);
    assert!(offset_of!(TermyEngineCell, background) == 16);
    assert!(offset_of!(TermyEngineCell, underline_color) == 20);
    assert!(offset_of!(TermyEngineCell, flags) == 24);
    assert!(offset_of!(TermyEngineCell, hyperlink) == 28);
};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TermyEngineHyperlink {
    pub id_ptr: *const u8,
    pub id_len: usize,
    pub uri_ptr: *const u8,
    pub uri_len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyEngineSpan {
    pub row: u16,
    pub start_col: u16,
    pub end_col: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TermyEngineCursor {
    pub row: u16,
    pub col: u16,
    pub shape: u8,
    pub visible: bool,
    pub blinking: bool,
    pub in_viewport: bool,
    pub on_wide_tail: bool,
}

/// Cells and their side tables. Every pointer borrows buffers owned by the
/// engine handle; see the header for how long they stay valid.
#[repr(C)]
#[derive(Debug)]
pub struct TermyEngineFrame {
    pub cols: u16,
    pub rows: u16,
    pub damage: u32,
    pub spans_ptr: *const TermyEngineSpan,
    pub spans_len: usize,
    pub cells_ptr: *const TermyEngineCell,
    pub cells_len: usize,
    pub graphemes_ptr: *const u32,
    pub graphemes_len: usize,
    pub hyperlinks_ptr: *const TermyEngineHyperlink,
    pub hyperlinks_len: usize,
    pub cursor: TermyEngineCursor,
    pub display_offset: u64,
    pub history_size: u64,
    pub generation: u64,
    pub palette_revision: u64,
}

impl Default for TermyEngineFrame {
    fn default() -> Self {
        Self {
            cols: 0,
            rows: 0,
            damage: DAMAGE_FULL,
            spans_ptr: ptr::null(),
            spans_len: 0,
            cells_ptr: ptr::null(),
            cells_len: 0,
            graphemes_ptr: ptr::null(),
            graphemes_len: 0,
            hyperlinks_ptr: ptr::null(),
            hyperlinks_len: 0,
            cursor: TermyEngineCursor::default(),
            display_offset: 0,
            history_size: 0,
            generation: 0,
            palette_revision: 0,
        }
    }
}

/// What the last frame showed, to detect changes that need a full frame.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Shown {
    cols: usize,
    rows: usize,
    alternate: bool,
    display_offset: usize,
    palette_revision: u64,
    reverse_video: bool,
}

#[derive(Default)]
pub(crate) struct FrameBuffers {
    cells: Vec<TermyEngineCell>,
    graphemes: Vec<u32>,
    hyperlinks: Vec<TermyEngineHyperlink>,
    // Keeps the link strings alive while the frame points at them.
    links: Vec<Arc<Hyperlink>>,
    link_indices: HashMap<usize, u32>,
    spans: Vec<TermyEngineSpan>,
    shown: Option<Shown>,
}

pub(crate) fn raw_color(color: Color) -> u32 {
    if let Some(index) = color.as_indexed() {
        0x0100_0000 | u32::from(index)
    } else if let Some((r, g, b)) = color.as_rgb() {
        0x0200_0000 | u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
    } else {
        0
    }
}

fn underline_code(underline: UnderlineStyle) -> u8 {
    match underline {
        UnderlineStyle::None => 0,
        UnderlineStyle::Single => 1,
        UnderlineStyle::Double => 2,
        UnderlineStyle::Curly => 3,
        UnderlineStyle::Dotted => 4,
        UnderlineStyle::Dashed => 5,
    }
}

impl FrameBuffers {
    /// Empty the buffers for a frame of at most `cells` cells. Capacity well
    /// past that is released: one large `termy_engine_read_lines` (up to
    /// 65,535 rows) would otherwise stay allocated for the handle's lifetime.
    fn clear(&mut self, cells: usize) {
        self.cells.clear();
        self.graphemes.clear();
        self.hyperlinks.clear();
        self.links.clear();
        self.link_indices.clear();
        self.spans.clear();
        // Small reads and ordinary frames keep their buffers, so alternating
        // between them does not reallocate.
        let keep = cells.saturating_mul(2).max(KEEP_CELLS);
        if self.cells.capacity() > keep {
            self.cells.shrink_to(cells);
        }
        // Clusters can carry many scalars per cell (combining marks).
        if self.graphemes.capacity() > keep.saturating_mul(16) {
            self.graphemes.shrink_to(cells);
        }
        if self.hyperlinks.capacity() > keep {
            self.hyperlinks.shrink_to(0);
            self.links.shrink_to(0);
            self.link_indices.shrink_to(0);
        }
    }

    fn link(&mut self, link: &Arc<Hyperlink>) -> u32 {
        let key = Arc::as_ptr(link).addr();
        if let Some(&index) = self.link_indices.get(&key) {
            return index;
        }
        self.hyperlinks.push(TermyEngineHyperlink {
            id_ptr: link.id.as_ptr(),
            id_len: link.id.len(),
            uri_ptr: link.uri.as_ptr(),
            uri_len: link.uri.len(),
        });
        self.links.push(Arc::clone(link));
        let index = self.hyperlinks.len() as u32;
        self.link_indices.insert(key, index);
        index
    }

    /// Append cells `start..end` of `line`, padding past its end with
    /// default blanks. `wrapped` marks a soft-wrapped row's last cell.
    pub(crate) fn push_line(
        &mut self,
        line: Option<Line<'_>>,
        start: usize,
        end: usize,
        cols: usize,
        wrapped: bool,
    ) {
        for col in start..end {
            let cell = line
                .as_ref()
                .and_then(|line| line.get(col).copied())
                .unwrap_or_default();
            let extra = line.as_ref().and_then(|line| line.extra(col));
            let mut out = self.cell(cell, extra);
            if wrapped && col + 1 == cols {
                out.flags |= FLAG_WRAPPED;
            }
            self.cells.push(out);
        }
    }

    fn cell(
        &mut self,
        cell: Cell,
        extra: Option<&termy_core::terminal_engine::CellExtra>,
    ) -> TermyEngineCell {
        let style = cell.style;
        let attributes = style.attributes;
        let mut flags = 0;
        for (bit, flag) in [
            (Style::BOLD, FLAG_BOLD),
            (Style::DIM, FLAG_FAINT),
            (Style::ITALIC, FLAG_ITALIC),
            (Style::INVERSE, FLAG_INVERSE),
            (Style::HIDDEN, FLAG_INVISIBLE),
            (Style::STRIKE, FLAG_STRIKETHROUGH),
            (Style::BLINK, FLAG_BLINK),
            (Style::PROTECTED, FLAG_PROTECTED),
        ] {
            if attributes & bit != 0 {
                flags |= flag;
            }
        }
        let width = if cell.flags & Cell::WIDE != 0 {
            CELL_WIDE
        } else if cell.flags & Cell::WIDE_SPACER != 0 {
            CELL_SPACER_TAIL
        } else if cell.flags & Cell::LEADING_WIDE_SPACER != 0 {
            CELL_SPACER_HEAD
        } else {
            CELL_NARROW
        };
        let grapheme_start = self.graphemes.len() as u32;
        let mut hyperlink = 0;
        let mut underline_color = 0;
        if let Some(extra) = extra {
            self.graphemes
                .extend(extra.combining.chars().map(u32::from));
            if let Some(link) = &extra.hyperlink {
                hyperlink = self.link(link);
            }
            underline_color = raw_color(extra.underline_color);
        }
        let grapheme_len = (self.graphemes.len() as u32 - grapheme_start) as u16;
        let styled = style != Style::default() || extra.is_some();
        if styled {
            flags |= FLAG_STYLED;
        }
        if !styled && cell.character == ' ' && width == CELL_NARROW {
            flags |= FLAG_BLANK;
        }
        TermyEngineCell {
            codepoint: u32::from(cell.character),
            grapheme_start,
            grapheme_len,
            width,
            underline: underline_code(style.underline),
            foreground: raw_color(style.foreground),
            background: raw_color(style.background),
            underline_color,
            flags,
            reserved: 0,
            hyperlink,
        }
    }

    fn fill_frame(&self, out: &mut TermyEngineFrame) {
        out.spans_ptr = self.spans.as_ptr();
        out.spans_len = self.spans.len();
        out.cells_ptr = self.cells.as_ptr();
        out.cells_len = self.cells.len();
        out.graphemes_ptr = self.graphemes.as_ptr();
        out.graphemes_len = self.graphemes.len();
        out.hyperlinks_ptr = self.hyperlinks.as_ptr();
        out.hyperlinks_len = self.hyperlinks.len();
    }
}

fn cursor(engine: &Engine) -> TermyEngineCursor {
    let cursor = engine.cursor();
    let rows = engine.size().rows;
    let row = cursor.row + engine.display_offset();
    let in_viewport = row < rows;
    let on_wide_tail = engine
        .line(cursor.row as i32)
        .and_then(|line| line.get(cursor.col).copied())
        .is_some_and(|cell| cell.flags & Cell::WIDE_SPACER != 0);
    TermyEngineCursor {
        row: row.min(u16::MAX as usize) as u16,
        col: cursor.col as u16,
        shape: match cursor.shape {
            CursorShape::Block => 0,
            CursorShape::Beam => 1,
            CursorShape::Underline => 2,
        },
        visible: cursor.visible && in_viewport,
        blinking: cursor.blinking,
        in_viewport,
        on_wide_tail,
    }
}

fn frame_header(engine: &Engine, out: &mut TermyEngineFrame) {
    let size = engine.size();
    out.cols = size.cols as u16;
    out.rows = size.rows as u16;
    out.cursor = cursor(engine);
    out.display_offset = engine.display_offset() as u64;
    out.history_size = engine.history_size() as u64;
    out.generation = engine.generation();
    out.palette_revision = engine.palette_revision();
}

fn build_frame(handle: &mut TermyEngine, force_full: bool, out: &mut TermyEngineFrame) {
    let engine = &mut handle.engine;
    let buffers = &mut handle.frame;
    let size = engine.size();
    let shown = Shown {
        cols: size.cols,
        rows: size.rows,
        alternate: engine.alternate_screen(),
        display_offset: engine.display_offset(),
        palette_revision: engine.palette_revision(),
        reverse_video: engine.modes().reverse_video,
    };
    let damage = engine.take_damage();
    // Engine damage is in live-screen rows; a scrolled-back viewport, or
    // any change to what the viewport shows, repaints everything.
    let full = force_full
        || buffers.shown != Some(shown)
        || matches!(damage, Damage::Full)
        || (shown.display_offset != 0
            && !matches!(&damage, Damage::Partial(spans) if spans.is_empty()));
    buffers.shown = Some(shown);
    buffers.clear(size.rows * size.cols);
    if full {
        for row in 0..size.rows {
            let line = engine.viewport_row(row);
            let wrapped =
                line.is_some() && engine.line_wrapped(row as i32 - engine.display_offset() as i32);
            buffers.push_line(line, 0, size.cols, size.cols, wrapped);
        }
        out.damage = DAMAGE_FULL;
    } else if let Damage::Partial(spans) = damage {
        for span in spans {
            let end = span.end.min(size.cols);
            if span.row >= size.rows || span.start >= end {
                continue;
            }
            buffers.spans.push(TermyEngineSpan {
                row: span.row as u16,
                start_col: span.start as u16,
                end_col: end as u16,
            });
            let wrapped = engine.viewport_row_wrapped(span.row);
            buffers.push_line(
                engine.viewport_row(span.row),
                span.start,
                end,
                size.cols,
                wrapped,
            );
        }
        out.damage = DAMAGE_PARTIAL;
    }
    frame_header(engine, out);
    buffers.fill_frame(out);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_frame(
    engine: *mut TermyEngine,
    force_full: bool,
    out_frame: *mut TermyEngineFrame,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable frame struct.
        let Some(out) = (unsafe { out_frame.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        *out = TermyEngineFrame::default();
        build_frame(engine, force_full, out);
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_read_lines(
    engine: *mut TermyEngine,
    first_line: i64,
    count: u32,
    out_frame: *mut TermyEngineFrame,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(handle) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable frame struct.
        let Some(out) = (unsafe { out_frame.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        *out = TermyEngineFrame::default();
        let engine = &handle.engine;
        let buffers = &mut handle.frame;
        // Reads don't consume damage, but the next frame must be full.
        buffers.shown = None;
        let cols = engine.size().cols;
        let first = first_line.max(-(engine.history_size() as i64));
        // `rows` is a u16 in the frame, so one call reads at most 65,535 rows.
        let last = (first_line.saturating_add(i64::from(count)))
            .min(engine.size().rows as i64)
            .min(first.saturating_add(i64::from(u16::MAX)));
        let lines = usize::try_from(last.saturating_sub(first)).unwrap_or(0);
        buffers.clear(lines.saturating_mul(cols));
        let mut rows = 0_u16;
        let mut line = first;
        // Packed history is decoded into scratch rather than through
        // `Engine::line`, which keeps a dense copy of every row it expands
        // until the next output: paging through scrollback would hold all of
        // it expanded.
        let mut scratch = Vec::new();
        while line < last {
            let number = line as i32;
            let wrapped = engine.line_wrapped(number);
            if engine
                .with_line(number, &mut scratch, |cells| {
                    buffers.push_line(Some(cells), 0, cols, cols, wrapped);
                })
                .is_none()
            {
                buffers.push_line(None, 0, cols, cols, wrapped);
            }
            rows += 1;
            line += 1;
        }
        frame_header(engine, out);
        out.rows = rows;
        out.damage = DAMAGE_FULL;
        buffers.fill_frame(out);
        TermyEngineStatus::Ok
    })
}
