use crate::render_metrics::{
    add_span_grid_paint_us, add_span_row_ops_rebuild_us, add_span_text_shaping_us,
    increment_grid_paint_count, increment_shape_line_calls, increment_shaped_line_cache_hit,
    increment_shaped_line_cache_miss, terminal_ui_render_metrics_enabled,
};
use gpui::{
    App, Bounds, Element, Font, FontFeatures, FontWeight, Hsla, IntoElement, PathBuilder, Pixels,
    ShapedLine, SharedString, Size, TextAlign, TextRun, UnderlineStyle, Window, point, px, quad,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc, time::Instant};
use termy_core::TerminalCursorStyle;

/// Info needed to render a single cell.
#[derive(Clone)]
pub struct CellRenderInfo {
    pub col: usize,
    pub row: usize,
    pub char: char,
    pub fg: Hsla,
    pub bg: Hsla,
    pub uses_terminal_default_bg: bool,
    pub bold: bool,
    pub render_text: bool,
    pub selected: bool,
    /// Part of the current (focused) search match
    pub search_current: bool,
    /// Part of any search match (but not current)
    pub search_match: bool,
}

/// Custom element for rendering the terminal grid.
pub type TerminalGridRow = Arc<Vec<CellRenderInfo>>;
pub type TerminalGridRows = Arc<Vec<TerminalGridRow>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TerminalGridPaintDamage {
    #[default]
    None,
    Full,
    Rows(Arc<[usize]>),
    /// Row damage with column bounds `(row, left_col_inclusive, right_col_inclusive)`.
    /// Emitted when alacritty reports partial damage with column-level granularity.
    RowRanges(Arc<[(usize, usize, usize)]>),
}

#[derive(Clone, Default)]
pub struct TerminalGridPaintCacheHandle(Rc<RefCell<TerminalGridPaintCache>>);

impl TerminalGridPaintCacheHandle {
    pub fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    #[cfg(any(test, debug_assertions))]
    #[doc(hidden)]
    pub fn debug_seed_rows_for_tests(&self, row_count: usize) {
        self.0.borrow_mut().row_ops = vec![CachedRowPaintOps::default(); row_count];
    }

    #[cfg(any(test, debug_assertions))]
    #[doc(hidden)]
    pub fn debug_row_cache_len_for_tests(&self) -> usize {
        self.0.borrow().row_ops.len()
    }
}

pub struct TerminalGrid {
    pub cells: TerminalGridRows,
    pub paint_cache: TerminalGridPaintCacheHandle,
    pub paint_damage: TerminalGridPaintDamage,
    pub cell_size: Size<Pixels>,
    pub cols: usize,
    pub rows: usize,
    /// Clear color used to reset the grid surface every frame.
    pub clear_bg: Hsla,
    pub terminal_surface_bg: Hsla,
    pub cursor_color: Hsla,
    pub selection_bg: Hsla,
    pub selection_fg: Hsla,
    pub search_match_bg: Hsla,
    pub search_current_bg: Hsla,
    pub hovered_link_range: Option<(usize, usize, usize)>,
    pub cursor_cell: Option<(usize, usize)>,
    pub cursor_visible: bool,
    pub font_family: SharedString,
    pub font_size: Pixels,
    pub cursor_style: TerminalCursorStyle,
}

impl IntoElement for TerminalGrid {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

// NOTE: We intentionally render Unicode block elements (U+2580..U+259F) as
// pixel-snapped quads instead of shaped font glyphs.
//
// Why:
// - Glyph rasterization anti-aliases the hard edges of chars like '▀'.
// - In transparent/layered terminal surfaces (GPUI terminals, e.g. Zed/opencode),
//   those semi-transparent edge pixels can show up as faint seams/lines.
// - Drawing exact geometry with snapped bounds gives deterministic, hard edges
//   and eliminates the artifact.
//
// NOTE: We also render Symbols for Legacy Computing sextant mosaics (U+1FB00..U+1FB3B)
// as quads. Terminal QR renderers (e.g. Expo SDK 55 `toqr`) use this range; most
// monospace fonts omit these codepoints and text shaping shows placeholders instead.
//
// NOTE: We also render Unicode box-drawing characters (U+2500..U+257F) as
// pixel-snapped quads instead of shaped font glyphs.
//
// Why:
// - Font glyphs are sized to the font's natural cell height, not the terminal's
//   cell height. When line_height > 1.0, this leaves visible gaps between rows.
// - Even at line_height = 1.0, built-in rendering gives crisper and more
//   consistent results across fonts, for the same reasons as block elements.
// - This matches Ghostty's unconditional sprite-rendering policy.
//
// Rounded corners (U+256D-U+2570) and diagonals (U+2571-U+2573) are handled as
// explicit stroked paths so they can render true curves/diagonals while still
// matching the built-in box-drawing stroke width.
const BOX_DRAWING_START: u32 = 0x2500;
const BOX_DRAWING_END: u32 = 0x257F;
const BLOCK_ELEMENTS_START: u32 = 0x2580;
const BLOCK_ELEMENTS_END: u32 = 0x259F;

/// Symbols for Legacy Computing mosaic (sextant) characters used by terminal QR
/// renderers such as `toqr` (Expo SDK 55+).
const SEXTANT_MOSAIC_START: u32 = 0x1FB00;
const SEXTANT_MOSAIC_END: u32 = 0x1FB3B;
const BRAILLE_PATTERNS_START: u32 = 0x2800;
const BRAILLE_PATTERNS_END: u32 = 0x28FF;
const QUAD_UPPER_LEFT: u8 = 0b0001;
const QUAD_UPPER_RIGHT: u8 = 0b0010;
const QUAD_LOWER_LEFT: u8 = 0b0100;
const QUAD_LOWER_RIGHT: u8 = 0b1000;

fn terminal_font_features() -> FontFeatures {
    // `force_width` is applied per shaped glyph. A standard `fi`/`fl` ligature
    // would therefore consume one cell even though its source occupies two.
    // GPUI's helper currently disables only `calt`; DirectWrite enables `liga`
    // and `clig` separately, so disable all three for a terminal grid.
    FontFeatures(Arc::new(vec![
        ("calt".to_string(), 0),
        ("liga".to_string(), 0),
        ("clig".to_string(), 0),
    ]))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BlockRectSpec {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    alpha: f32,
}

impl BlockRectSpec {
    const fn new(left: f32, top: f32, right: f32, bottom: f32, alpha: f32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
            alpha,
        }
    }
}

const EMPTY_BLOCK_RECT: BlockRectSpec = BlockRectSpec::new(0.0, 0.0, 0.0, 0.0, 0.0);

/// Collected set of cell-relative rectangles that compose a single block-element
/// or box-drawing character. Fixed-capacity array (max 8 rects) to avoid heap
/// allocation — the most complex box-drawing connectors expand to 8 rects
/// before overlapping collinear runs are merged back together.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BlockElementGeometry {
    rects: [BlockRectSpec; 8],
    rect_count: usize,
}

impl BlockElementGeometry {
    const fn empty() -> Self {
        Self {
            rects: [EMPTY_BLOCK_RECT; 8],
            rect_count: 0,
        }
    }

    const fn one(rect: BlockRectSpec) -> Self {
        Self {
            rects: [
                rect,
                EMPTY_BLOCK_RECT,
                EMPTY_BLOCK_RECT,
                EMPTY_BLOCK_RECT,
                EMPTY_BLOCK_RECT,
                EMPTY_BLOCK_RECT,
                EMPTY_BLOCK_RECT,
                EMPTY_BLOCK_RECT,
            ],
            rect_count: 1,
        }
    }

    fn push_rect(&mut self, rect: BlockRectSpec) {
        debug_assert!(
            self.rect_count < self.rects.len(),
            "box geometry exceeded rect capacity"
        );
        if self.rect_count >= self.rects.len() {
            // Preserve release stability if a future mapping regression exceeds
            // the fixed connector rect budget.
            return;
        }
        self.rects[self.rect_count] = rect;
        self.rect_count += 1;
    }

    fn rects(&self) -> &[BlockRectSpec] {
        &self.rects[..self.rect_count]
    }

    /// Merges any pair of rects that share the same axis track and overlap or touch.
    ///
    /// Two rects are "collinear" if they have the same left/right (vertical track)
    /// or the same top/bottom (horizontal track) and their perpendicular extents
    /// overlap within `EPSILON`. The merged rect takes the union of both bounding
    /// boxes and the maximum alpha.
    ///
    /// Called once after all arms of a box-drawing connector have been pushed, so
    /// that a simple light-cross (which pushes one vertical + one horizontal rect
    /// overlapping at center) stays as two rects rather than fragmenting into four.
    fn merge_collinear_overlaps(&mut self) {
        const EPSILON: f32 = 1e-6;

        let mut i = 0;
        while i < self.rect_count {
            let mut j = i + 1;
            while j < self.rect_count {
                let a = self.rects[i];
                let b = self.rects[j];

                let same_vertical_track = (a.left - b.left).abs() <= EPSILON
                    && (a.right - b.right).abs() <= EPSILON
                    && a.top <= b.bottom + EPSILON
                    && b.top <= a.bottom + EPSILON;
                let same_horizontal_track = (a.top - b.top).abs() <= EPSILON
                    && (a.bottom - b.bottom).abs() <= EPSILON
                    && a.left <= b.right + EPSILON
                    && b.left <= a.right + EPSILON;

                if same_vertical_track || same_horizontal_track {
                    self.rects[i] = BlockRectSpec::new(
                        a.left.min(b.left),
                        a.top.min(b.top),
                        a.right.max(b.right),
                        a.bottom.max(b.bottom),
                        a.alpha.max(b.alpha),
                    );

                    for k in j..(self.rect_count - 1) {
                        self.rects[k] = self.rects[k + 1];
                    }
                    self.rects[self.rect_count - 1] = EMPTY_BLOCK_RECT;
                    self.rect_count -= 1;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }
}

/// Stroke weight for one arm of a box-drawing connector.
///
/// Maps directly to the Unicode box-drawing naming convention: light strokes are
/// 1x the base width, heavy strokes are 2x, and double strokes are two
/// parallel light lines separated by one light-width gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoxLineStyle {
    None,
    Light,
    Heavy,
    Double,
}

impl BoxLineStyle {
    fn is_double(self) -> bool {
        self == Self::Double
    }

    fn is_heavy(self) -> bool {
        self == Self::Heavy
    }
}

/// Four-arm style descriptor for a rectangular box-drawing character.
///
/// Each field declares whether the character extends a line in that cardinal
/// direction and, if so, which stroke weight it uses. `box_draw_geometry`
/// converts this into concrete pixel rectangles sized to the terminal cell.
///
/// Rounded corners (U+256D-U+2570) and diagonals (U+2571-U+2573) are not
/// representable here and return `None` from `box_draw_segments`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoxDrawSegments {
    up: BoxLineStyle,
    down: BoxLineStyle,
    left: BoxLineStyle,
    right: BoxLineStyle,
}

impl BoxDrawSegments {
    const fn new(
        up: BoxLineStyle,
        down: BoxLineStyle,
        left: BoxLineStyle,
        right: BoxLineStyle,
    ) -> Self {
        Self {
            up,
            down,
            left,
            right,
        }
    }
}

#[derive(Clone)]
struct TextBatch {
    start_col: usize,
    #[allow(dead_code)]
    row: usize,
    /// Text content. Stored as `SharedString` so that clones during text
    /// shaping are cheap refcount bumps instead of heap copies.
    text: SharedString,
    bold: bool,
    fg: Hsla,
    underline: Option<UnderlineStyle>,
    cell_len: usize,
}

#[derive(Clone, Copy)]
struct BlockDraw {
    #[cfg_attr(not(test), allow(dead_code))]
    row: usize,
    col: usize,
    geometry: BlockElementGeometry,
    fg: Hsla,
}

#[derive(Clone, Copy)]
struct SextantDraw {
    #[cfg_attr(not(test), allow(dead_code))]
    row: usize,
    col: usize,
    packed: u8,
    fg: Hsla,
}

/// Deferred paint operation for a rounded-corner box-drawing glyph (U+256D-U+2570).
///
/// Unlike `BlockDraw`, these are painted as stroked cubic Bézier paths rather
/// than axis-aligned quads, so the glyph codepoint is stored and resolved to a
/// path at paint time.
#[derive(Clone, Copy)]
struct RoundedCornerDraw {
    #[allow(dead_code)]
    row: usize,
    col: usize,
    glyph: char,
    fg: Hsla,
}

/// Deferred paint operation for a diagonal box-drawing glyph (U+2571-U+2573).
///
/// Diagonals are painted as stroked straight lines with slope-dependent
/// overshoot past cell boundaries to avoid pixel gaps at adjacent-cell seams.
#[derive(Clone, Copy)]
struct DiagonalDraw {
    #[allow(dead_code)]
    row: usize,
    col: usize,
    glyph: char,
    fg: Hsla,
}

#[derive(Clone)]
enum TextDrawOp {
    Batch(TextBatch),
    Block(BlockDraw),
    Sextant(SextantDraw),
    RoundedCorner(RoundedCornerDraw),
    Diagonal(DiagonalDraw),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BackgroundSpan {
    start_col: usize,
    end_col_exclusive: usize,
    color: Hsla,
}

/// Cached paint operations for a single terminal row.
///
/// Rebuilt when the row is in the dirty set; otherwise reused across frames.
/// `shaped_lines` is parallel to `draw_ops` — each `TextDrawOp::Batch` has a
/// corresponding `Some(Rc<ShapedLine>)` (populated on first paint or reused from
/// a previous frame), while non-text entries have a pointer-sized `None`.
#[derive(Clone, Default)]
struct CachedRowPaintOps {
    background_spans: Vec<BackgroundSpan>,
    draw_ops: Vec<TextDrawOp>,
    shaped_lines: Vec<Option<Rc<ShapedLine>>>,
}

#[derive(Clone, Debug, PartialEq)]
struct GridPaintStyleKey {
    cols: usize,
    rows: usize,
    cell_width_bits: u32,
    cell_height_bits: u32,
    clear_bg: [u32; 4],
    terminal_surface_bg: [u32; 4],
    selection_bg: [u32; 4],
    selection_fg: [u32; 4],
    search_match_bg: [u32; 4],
    search_current_bg: [u32; 4],
    cursor_style: TerminalCursorStyle,
    font_family: SharedString,
    font_size_bits: u32,
}

#[derive(Default)]
struct TerminalGridPaintCache {
    row_ops: Vec<CachedRowPaintOps>,
    style_key: Option<GridPaintStyleKey>,
    last_cursor_cell: Option<(usize, usize)>,
    last_cursor_visible: bool,
    last_hovered_link_range: Option<(usize, usize, usize)>,
    /// Per-pass scratch for dirty row indices. Reused to avoid allocating a new
    /// Vec/Arc for every paint pass.
    dirty_rows: Vec<usize>,
    /// Per-pass scratch: `Some((left, right))` if only that column range is dirty for the row.
    /// `None` means full-row damage (cursor/hover transitions, or no damage info available).
    /// Cleared and repopulated at the start of every paint pass.
    dirty_col_ranges: Vec<Option<(usize, usize)>>,
    /// Per-style cache: maps hsla_bits(cell.bg) → resolved background fill color.
    /// Avoids redundant float comparisons when many cells share the same default background.
    /// Cleared whenever the style key changes.
    color_cache: HashMap<[u32; 4], Option<Hsla>>,
    /// Cached Font objects, rebuilt only when style_key changes.
    cached_font_normal: Option<Font>,
    cached_font_bold: Option<Font>,
}

impl TerminalGridPaintCache {
    fn clear(&mut self) {
        // This is an eviction path, not a per-frame reset. Drop the backing
        // allocations as well as their contents so hidden tabs release shaped
        // lines, draw ops, and color-cache capacity immediately.
        *self = Self::default();
    }

    fn ensure_row_capacity(&mut self, row_count: usize) {
        let old_len = self.row_ops.len();
        if old_len < row_count {
            // Growing: keep existing cached rows, add new default rows
            self.row_ops
                .resize_with(row_count, CachedRowPaintOps::default);
        } else if old_len > row_count {
            // Shrinking: truncate, existing rows still valid for their indices
            self.row_ops.truncate(row_count);
        }
        // dirty_col_ranges is per-pass scratch — resize and reset every frame.
        // Use resize + fill to reuse the existing allocation when row count is stable.
        self.dirty_col_ranges.resize(row_count, None);
        self.dirty_col_ranges.fill(None);
    }
}

#[derive(Clone, Copy)]
struct TextBatchKey {
    bold: bool,
    fg: Hsla,
}

/// Temporary mutable builder for a text batch. Collects chars into a String,
/// then converts to the immutable `TextBatch` (with `SharedString`) on finalize.
struct TextBatchBuilder {
    start_col: usize,
    row: usize,
    text: String,
    bold: bool,
    fg: Hsla,
    underline: Option<UnderlineStyle>,
    cell_len: usize,
}

impl TextBatchBuilder {
    fn new(
        start_col: usize,
        row: usize,
        initial_char: char,
        key: TextBatchKey,
        underline: Option<UnderlineStyle>,
    ) -> Self {
        let mut text = String::with_capacity(16);
        text.push(initial_char);
        Self {
            start_col,
            row,
            text,
            bold: key.bold,
            fg: key.fg,
            underline,
            cell_len: 1,
        }
    }

    fn can_append(
        &self,
        col: usize,
        row: usize,
        key: TextBatchKey,
        underline: &Option<UnderlineStyle>,
    ) -> bool {
        self.row == row
            && self.start_col + self.cell_len == col
            && self.bold == key.bold
            && self.fg == key.fg
            && self.underline == *underline
    }

    fn append_char(&mut self, c: char) {
        self.text.push(c);
        self.cell_len += 1;
    }

    fn finalize(self) -> TextBatch {
        TextBatch {
            start_col: self.start_col,
            row: self.row,
            text: SharedString::from(self.text),
            bold: self.bold,
            fg: self.fg,
            underline: self.underline,
            cell_len: self.cell_len,
        }
    }
}

fn full_cell_rect(alpha: f32) -> BlockRectSpec {
    BlockRectSpec::new(0.0, 0.0, 1.0, 1.0, alpha)
}

fn vertical_fill_from_bottom(fraction: f32) -> BlockElementGeometry {
    BlockElementGeometry::one(BlockRectSpec::new(0.0, 1.0 - fraction, 1.0, 1.0, 1.0))
}

fn horizontal_fill_from_left(fraction: f32) -> BlockElementGeometry {
    BlockElementGeometry::one(BlockRectSpec::new(0.0, 0.0, fraction, 1.0, 1.0))
}

fn quadrants(mask: u8) -> BlockElementGeometry {
    let mut rects = [EMPTY_BLOCK_RECT; 8];
    let mut count = 0;

    if mask & QUAD_UPPER_LEFT != 0 {
        rects[count] = BlockRectSpec::new(0.0, 0.0, 0.5, 0.5, 1.0);
        count += 1;
    }
    if mask & QUAD_UPPER_RIGHT != 0 {
        rects[count] = BlockRectSpec::new(0.5, 0.0, 1.0, 0.5, 1.0);
        count += 1;
    }
    if mask & QUAD_LOWER_LEFT != 0 {
        rects[count] = BlockRectSpec::new(0.0, 0.5, 0.5, 1.0, 1.0);
        count += 1;
    }
    if mask & QUAD_LOWER_RIGHT != 0 {
        rects[count] = BlockRectSpec::new(0.5, 0.5, 1.0, 1.0, 1.0);
        count += 1;
    }

    BlockElementGeometry {
        rects,
        rect_count: count,
    }
}

fn block_element_geometry(c: char) -> Option<BlockElementGeometry> {
    let codepoint = c as u32;
    if !(BLOCK_ELEMENTS_START..=BLOCK_ELEMENTS_END).contains(&codepoint) {
        return None;
    }

    Some(match c {
        '\u{2580}' => BlockElementGeometry::one(BlockRectSpec::new(0.0, 0.0, 1.0, 0.5, 1.0)),
        '\u{2581}' => vertical_fill_from_bottom(1.0 / 8.0),
        '\u{2582}' => vertical_fill_from_bottom(2.0 / 8.0),
        '\u{2583}' => vertical_fill_from_bottom(3.0 / 8.0),
        '\u{2584}' => vertical_fill_from_bottom(4.0 / 8.0),
        '\u{2585}' => vertical_fill_from_bottom(5.0 / 8.0),
        '\u{2586}' => vertical_fill_from_bottom(6.0 / 8.0),
        '\u{2587}' => vertical_fill_from_bottom(7.0 / 8.0),
        '\u{2588}' => BlockElementGeometry::one(full_cell_rect(1.0)),
        '\u{2589}' => horizontal_fill_from_left(7.0 / 8.0),
        '\u{258A}' => horizontal_fill_from_left(6.0 / 8.0),
        '\u{258B}' => horizontal_fill_from_left(5.0 / 8.0),
        '\u{258C}' => horizontal_fill_from_left(4.0 / 8.0),
        '\u{258D}' => horizontal_fill_from_left(3.0 / 8.0),
        '\u{258E}' => horizontal_fill_from_left(2.0 / 8.0),
        '\u{258F}' => horizontal_fill_from_left(1.0 / 8.0),
        '\u{2590}' => BlockElementGeometry::one(BlockRectSpec::new(0.5, 0.0, 1.0, 1.0, 1.0)),
        '\u{2591}' => BlockElementGeometry::one(full_cell_rect(0.25)),
        '\u{2592}' => BlockElementGeometry::one(full_cell_rect(0.50)),
        '\u{2593}' => BlockElementGeometry::one(full_cell_rect(0.75)),
        '\u{2594}' => BlockElementGeometry::one(BlockRectSpec::new(0.0, 0.0, 1.0, 1.0 / 8.0, 1.0)),
        '\u{2595}' => BlockElementGeometry::one(BlockRectSpec::new(7.0 / 8.0, 0.0, 1.0, 1.0, 1.0)),
        '\u{2596}' => quadrants(QUAD_LOWER_LEFT),
        '\u{2597}' => quadrants(QUAD_LOWER_RIGHT),
        '\u{2598}' => quadrants(QUAD_UPPER_LEFT),
        '\u{2599}' => quadrants(QUAD_UPPER_LEFT | QUAD_LOWER_LEFT | QUAD_LOWER_RIGHT),
        '\u{259A}' => quadrants(QUAD_UPPER_LEFT | QUAD_LOWER_RIGHT),
        '\u{259B}' => quadrants(QUAD_UPPER_LEFT | QUAD_UPPER_RIGHT | QUAD_LOWER_LEFT),
        '\u{259C}' => quadrants(QUAD_UPPER_LEFT | QUAD_UPPER_RIGHT | QUAD_LOWER_RIGHT),
        '\u{259D}' => quadrants(QUAD_UPPER_RIGHT),
        '\u{259E}' => quadrants(QUAD_UPPER_RIGHT | QUAD_LOWER_LEFT),
        '\u{259F}' => quadrants(QUAD_UPPER_RIGHT | QUAD_LOWER_LEFT | QUAD_LOWER_RIGHT),
        _ => return None,
    })
}

fn reverse_lower_six_bits(value: u8) -> u8 {
    ((value & 0b00_0001) << 5)
        | ((value & 0b00_0010) << 3)
        | ((value & 0b00_0100) << 1)
        | ((value & 0b00_1000) >> 1)
        | ((value & 0b01_0000) >> 3)
        | ((value & 0b10_0000) >> 5)
}

fn sextant_char_to_packed(ch: char) -> Option<u8> {
    let codepoint = ch as u32;
    if !(SEXTANT_MOSAIC_START..=SEXTANT_MOSAIC_END).contains(&codepoint) {
        return None;
    }

    let offset = codepoint - SEXTANT_MOSAIC_START;
    let sextant = (offset + 1 + u32::from(offset >= 20) + u32::from(offset >= 40)) as u8;
    Some(reverse_lower_six_bits(sextant) ^ 0b11_1111)
}

fn braille_geometry(c: char) -> Option<BlockElementGeometry> {
    let codepoint = c as u32;
    if !is_braille_pattern_char(c) {
        return None;
    }

    let pattern = (codepoint - BRAILLE_PATTERNS_START) as u8;
    if pattern == 0 {
        return None;
    }

    // Braille QR output typically uses the Unicode braille block. Render it as
    // explicit geometry so output remains readable even when the configured
    // terminal font lacks braille glyph coverage.
    const DOT_WIDTH: f32 = 0.24;
    const DOT_HEIGHT: f32 = 0.16;
    const LEFT_X: f32 = 0.22;
    const RIGHT_X: f32 = 0.64;
    const ROW_Y: [f32; 4] = [0.08, 0.31, 0.54, 0.77];
    const DOT_MASKS: [(u8, f32, f32); 8] = [
        (0b0000_0001, LEFT_X, ROW_Y[0]),
        (0b0000_0010, LEFT_X, ROW_Y[1]),
        (0b0000_0100, LEFT_X, ROW_Y[2]),
        (0b0100_0000, LEFT_X, ROW_Y[3]),
        (0b0000_1000, RIGHT_X, ROW_Y[0]),
        (0b0001_0000, RIGHT_X, ROW_Y[1]),
        (0b0010_0000, RIGHT_X, ROW_Y[2]),
        (0b1000_0000, RIGHT_X, ROW_Y[3]),
    ];

    let mut geometry = BlockElementGeometry::empty();
    for (mask, left, top) in DOT_MASKS {
        if pattern & mask == 0 {
            continue;
        }
        geometry.push_rect(BlockRectSpec::new(
            left,
            top,
            (left + DOT_WIDTH).min(1.0),
            (top + DOT_HEIGHT).min(1.0),
            1.0,
        ));
    }
    Some(geometry)
}

fn is_braille_pattern_char(c: char) -> bool {
    (BRAILLE_PATTERNS_START..=BRAILLE_PATTERNS_END).contains(&(c as u32))
}

fn should_render_braille_as_geometry(row_cells: &[CellRenderInfo], index: usize) -> bool {
    let Some(cell) = row_cells.get(index) else {
        return false;
    };
    if !is_braille_pattern_char(cell.char) {
        return false;
    }

    // QR output is emitted as longer contiguous braille runs. Some CLI loading
    // spinners use two cells, so only switch to geometry for runs of three or more.
    let is_braille_at = |candidate: usize| {
        row_cells
            .get(candidate)
            .is_some_and(|neighbor| is_braille_pattern_char(neighbor.char))
    };

    (index >= 2 && is_braille_at(index - 2) && is_braille_at(index - 1))
        || (index >= 1 && is_braille_at(index - 1) && is_braille_at(index + 1))
        || (is_braille_at(index + 1) && is_braille_at(index + 2))
}

const fn box_segments(
    up: BoxLineStyle,
    down: BoxLineStyle,
    left: BoxLineStyle,
    right: BoxLineStyle,
) -> BoxDrawSegments {
    BoxDrawSegments::new(up, down, left, right)
}

/// Looks up the four-arm style descriptor for a box-drawing codepoint.
///
/// Returns `None` for rounded corners (U+256D-U+2570), diagonals
/// (U+2571-U+2573), and anything outside U+2500..U+257F.
#[allow(clippy::too_many_lines)]
fn box_draw_segments(c: char) -> Option<BoxDrawSegments> {
    use BoxLineStyle::{Double, Heavy, Light, None as Empty};

    let codepoint = c as u32;
    if !(BOX_DRAWING_START..=BOX_DRAWING_END).contains(&codepoint) {
        return None;
    }

    Some(match c {
        '\u{2500}' | '\u{2504}' | '\u{2508}' | '\u{254C}' => {
            box_segments(Empty, Empty, Light, Light)
        }
        '\u{2501}' | '\u{2505}' | '\u{2509}' | '\u{254D}' => {
            box_segments(Empty, Empty, Heavy, Heavy)
        }
        '\u{2502}' | '\u{2506}' | '\u{250A}' | '\u{254E}' => {
            box_segments(Light, Light, Empty, Empty)
        }
        '\u{2503}' | '\u{2507}' | '\u{250B}' | '\u{254F}' => {
            box_segments(Heavy, Heavy, Empty, Empty)
        }
        '\u{250C}' => box_segments(Empty, Light, Empty, Light),
        '\u{250D}' => box_segments(Empty, Light, Empty, Heavy),
        '\u{250E}' => box_segments(Empty, Heavy, Empty, Light),
        '\u{250F}' => box_segments(Empty, Heavy, Empty, Heavy),
        '\u{2510}' => box_segments(Empty, Light, Light, Empty),
        '\u{2511}' => box_segments(Empty, Light, Heavy, Empty),
        '\u{2512}' => box_segments(Empty, Heavy, Light, Empty),
        '\u{2513}' => box_segments(Empty, Heavy, Heavy, Empty),
        '\u{2514}' => box_segments(Light, Empty, Empty, Light),
        '\u{2515}' => box_segments(Light, Empty, Empty, Heavy),
        '\u{2516}' => box_segments(Heavy, Empty, Empty, Light),
        '\u{2517}' => box_segments(Heavy, Empty, Empty, Heavy),
        '\u{2518}' => box_segments(Light, Empty, Light, Empty),
        '\u{2519}' => box_segments(Light, Empty, Heavy, Empty),
        '\u{251A}' => box_segments(Heavy, Empty, Light, Empty),
        '\u{251B}' => box_segments(Heavy, Empty, Heavy, Empty),
        '\u{251C}' => box_segments(Light, Light, Empty, Light),
        '\u{251D}' => box_segments(Light, Light, Empty, Heavy),
        '\u{251E}' => box_segments(Heavy, Light, Empty, Light),
        '\u{251F}' => box_segments(Light, Heavy, Empty, Light),
        '\u{2520}' => box_segments(Heavy, Heavy, Empty, Light),
        '\u{2521}' => box_segments(Light, Heavy, Empty, Heavy),
        '\u{2522}' => box_segments(Heavy, Light, Empty, Heavy),
        '\u{2523}' => box_segments(Heavy, Heavy, Empty, Heavy),
        '\u{2524}' => box_segments(Light, Light, Light, Empty),
        '\u{2525}' => box_segments(Light, Light, Heavy, Empty),
        '\u{2526}' => box_segments(Heavy, Light, Light, Empty),
        '\u{2527}' => box_segments(Light, Heavy, Light, Empty),
        '\u{2528}' => box_segments(Heavy, Heavy, Light, Empty),
        '\u{2529}' => box_segments(Light, Heavy, Heavy, Empty),
        '\u{252A}' => box_segments(Heavy, Light, Heavy, Empty),
        '\u{252B}' => box_segments(Heavy, Heavy, Heavy, Empty),
        '\u{252C}' => box_segments(Empty, Light, Light, Light),
        '\u{252D}' => box_segments(Empty, Light, Heavy, Light),
        '\u{252E}' => box_segments(Empty, Light, Light, Heavy),
        '\u{252F}' => box_segments(Empty, Light, Heavy, Heavy),
        '\u{2530}' => box_segments(Empty, Heavy, Light, Light),
        '\u{2531}' => box_segments(Empty, Heavy, Heavy, Light),
        '\u{2532}' => box_segments(Empty, Heavy, Light, Heavy),
        '\u{2533}' => box_segments(Empty, Heavy, Heavy, Heavy),
        '\u{2534}' => box_segments(Light, Empty, Light, Light),
        '\u{2535}' => box_segments(Light, Empty, Heavy, Light),
        '\u{2536}' => box_segments(Light, Empty, Light, Heavy),
        '\u{2537}' => box_segments(Light, Empty, Heavy, Heavy),
        '\u{2538}' => box_segments(Heavy, Empty, Light, Light),
        '\u{2539}' => box_segments(Heavy, Empty, Heavy, Light),
        '\u{253A}' => box_segments(Heavy, Empty, Light, Heavy),
        '\u{253B}' => box_segments(Heavy, Empty, Heavy, Heavy),
        '\u{253C}' => box_segments(Light, Light, Light, Light),
        '\u{253D}' => box_segments(Light, Light, Heavy, Light),
        '\u{253E}' => box_segments(Light, Light, Light, Heavy),
        '\u{253F}' => box_segments(Light, Light, Heavy, Heavy),
        '\u{2540}' => box_segments(Heavy, Light, Light, Light),
        '\u{2541}' => box_segments(Light, Heavy, Light, Light),
        '\u{2542}' => box_segments(Heavy, Heavy, Light, Light),
        '\u{2543}' => box_segments(Heavy, Light, Heavy, Light),
        '\u{2544}' => box_segments(Heavy, Light, Light, Heavy),
        '\u{2545}' => box_segments(Light, Heavy, Heavy, Light),
        '\u{2546}' => box_segments(Light, Heavy, Light, Heavy),
        '\u{2547}' => box_segments(Light, Heavy, Heavy, Heavy),
        '\u{2548}' => box_segments(Heavy, Light, Heavy, Heavy),
        '\u{2549}' => box_segments(Heavy, Heavy, Heavy, Light),
        '\u{254A}' => box_segments(Heavy, Heavy, Light, Heavy),
        '\u{254B}' => box_segments(Heavy, Heavy, Heavy, Heavy),
        '\u{2550}' => box_segments(Empty, Empty, Double, Double),
        '\u{2551}' => box_segments(Double, Double, Empty, Empty),
        '\u{2552}' => box_segments(Empty, Light, Empty, Double),
        '\u{2553}' => box_segments(Empty, Double, Empty, Light),
        '\u{2554}' => box_segments(Empty, Double, Empty, Double),
        '\u{2555}' => box_segments(Empty, Light, Double, Empty),
        '\u{2556}' => box_segments(Empty, Double, Light, Empty),
        '\u{2557}' => box_segments(Empty, Double, Double, Empty),
        '\u{2558}' => box_segments(Light, Empty, Empty, Double),
        '\u{2559}' => box_segments(Double, Empty, Empty, Light),
        '\u{255A}' => box_segments(Double, Empty, Empty, Double),
        '\u{255B}' => box_segments(Light, Empty, Double, Empty),
        '\u{255C}' => box_segments(Double, Empty, Light, Empty),
        '\u{255D}' => box_segments(Double, Empty, Double, Empty),
        '\u{255E}' => box_segments(Light, Light, Empty, Double),
        '\u{255F}' => box_segments(Double, Double, Empty, Light),
        '\u{2560}' => box_segments(Double, Double, Empty, Double),
        '\u{2561}' => box_segments(Light, Light, Double, Empty),
        '\u{2562}' => box_segments(Double, Double, Light, Empty),
        '\u{2563}' => box_segments(Double, Double, Double, Empty),
        '\u{2564}' => box_segments(Empty, Light, Double, Double),
        '\u{2565}' => box_segments(Empty, Double, Light, Light),
        '\u{2566}' => box_segments(Empty, Double, Double, Double),
        '\u{2567}' => box_segments(Light, Empty, Double, Double),
        '\u{2568}' => box_segments(Double, Empty, Light, Light),
        '\u{2569}' => box_segments(Double, Empty, Double, Double),
        '\u{256A}' => box_segments(Light, Light, Double, Double),
        '\u{256B}' => box_segments(Double, Double, Light, Light),
        '\u{256C}' => box_segments(Double, Double, Double, Double),
        '\u{256D}'..='\u{2570}' => return None,
        '\u{2571}'..='\u{2573}' => return None,
        '\u{2574}' => box_segments(Empty, Empty, Light, Empty),
        '\u{2575}' => box_segments(Light, Empty, Empty, Empty),
        '\u{2576}' => box_segments(Empty, Empty, Empty, Light),
        '\u{2577}' => box_segments(Empty, Light, Empty, Empty),
        '\u{2578}' => box_segments(Empty, Empty, Heavy, Empty),
        '\u{2579}' => box_segments(Heavy, Empty, Empty, Empty),
        '\u{257A}' => box_segments(Empty, Empty, Empty, Heavy),
        '\u{257B}' => box_segments(Empty, Heavy, Empty, Empty),
        '\u{257C}' => box_segments(Empty, Empty, Light, Heavy),
        '\u{257D}' => box_segments(Light, Heavy, Empty, Empty),
        '\u{257E}' => box_segments(Empty, Empty, Heavy, Light),
        '\u{257F}' => box_segments(Heavy, Light, Empty, Empty),
        _ => return None,
    })
}

/// Pushes a rectangle into `geometry`, converting absolute pixel coordinates to
/// cell-relative fractions (0.0..1.0). Clamps to cell bounds and silently
/// discards zero-area results.
fn push_box_rect_px(
    geometry: &mut BlockElementGeometry,
    left_px: f32,
    top_px: f32,
    right_px: f32,
    bottom_px: f32,
    cell_width: f32,
    cell_height: f32,
) {
    let left = left_px.clamp(0.0, cell_width);
    let right = right_px.clamp(0.0, cell_width);
    let top = top_px.clamp(0.0, cell_height);
    let bottom = bottom_px.clamp(0.0, cell_height);

    if right <= left || bottom <= top {
        return;
    }

    geometry.push_rect(BlockRectSpec::new(
        left / cell_width,
        top / cell_height,
        right / cell_width,
        bottom / cell_height,
        1.0,
    ));
}

/// Converts a `BoxDrawSegments` descriptor into pixel-snappable rectangles using
/// Ghostty's `linesChar` edge placement. Each arm is built independently, then
/// overlapping collinear runs are merged back together so simple glyphs stay
/// compact while mixed light/heavy/double connectors keep Ghostty's join logic.
fn box_draw_geometry(
    segments: BoxDrawSegments,
    cell_width: f32,
    cell_height: f32,
    font_size: f32,
) -> BlockElementGeometry {
    use BoxLineStyle::{Double, Heavy, Light, None as Empty};

    let light_px = (font_size * 0.0675).ceil().max(1.0);
    let heavy_px = light_px * 2.0;

    let h_light_top = ((cell_height - light_px).max(0.0)) / 2.0;
    let h_light_bottom = (h_light_top + light_px).min(cell_height);
    let h_heavy_top = ((cell_height - heavy_px).max(0.0)) / 2.0;
    let h_heavy_bottom = (h_heavy_top + heavy_px).min(cell_height);
    let h_double_top = (h_light_top - light_px).max(0.0);
    let h_double_bottom = (h_light_bottom + light_px).min(cell_height);

    let v_light_left = ((cell_width - light_px).max(0.0)) / 2.0;
    let v_light_right = (v_light_left + light_px).min(cell_width);
    let v_heavy_left = ((cell_width - heavy_px).max(0.0)) / 2.0;
    let v_heavy_right = (v_heavy_left + heavy_px).min(cell_width);
    let v_double_left = (v_light_left - light_px).max(0.0);
    let v_double_right = (v_light_right + light_px).min(cell_width);

    let up_bottom = if segments.left.is_heavy() || segments.right.is_heavy() {
        h_heavy_bottom
    } else if segments.left != segments.right || segments.down == segments.up {
        if segments.left.is_double() || segments.right.is_double() {
            h_double_bottom
        } else {
            h_light_bottom
        }
    } else if segments.left == Empty && segments.right == Empty {
        h_light_bottom
    } else {
        h_light_top
    };

    let down_top = if segments.left.is_heavy() || segments.right.is_heavy() {
        h_heavy_top
    } else if segments.left != segments.right || segments.up == segments.down {
        if segments.left.is_double() || segments.right.is_double() {
            h_double_top
        } else {
            h_light_top
        }
    } else if segments.left == Empty && segments.right == Empty {
        h_light_top
    } else {
        h_light_bottom
    };

    let left_right = if segments.up.is_heavy() || segments.down.is_heavy() {
        v_heavy_right
    } else if segments.up != segments.down || segments.left == segments.right {
        if segments.up.is_double() || segments.down.is_double() {
            v_double_right
        } else {
            v_light_right
        }
    } else if segments.up == Empty && segments.down == Empty {
        v_light_right
    } else {
        v_light_left
    };

    let right_left = if segments.up.is_heavy() || segments.down.is_heavy() {
        v_heavy_left
    } else if segments.up != segments.down || segments.right == segments.left {
        if segments.up.is_double() || segments.down.is_double() {
            v_double_left
        } else {
            v_light_left
        }
    } else if segments.up == Empty && segments.down == Empty {
        v_light_left
    } else {
        v_light_right
    };

    let mut geometry = BlockElementGeometry::empty();

    match segments.up {
        Empty => {}
        Light => push_box_rect_px(
            &mut geometry,
            v_light_left,
            0.0,
            v_light_right,
            up_bottom,
            cell_width,
            cell_height,
        ),
        Heavy => push_box_rect_px(
            &mut geometry,
            v_heavy_left,
            0.0,
            v_heavy_right,
            up_bottom,
            cell_width,
            cell_height,
        ),
        Double => {
            let left_bottom = if segments.left == Double {
                h_light_top
            } else {
                up_bottom
            };
            let right_bottom = if segments.right == Double {
                h_light_top
            } else {
                up_bottom
            };
            push_box_rect_px(
                &mut geometry,
                v_double_left,
                0.0,
                v_light_left,
                left_bottom,
                cell_width,
                cell_height,
            );
            push_box_rect_px(
                &mut geometry,
                v_light_right,
                0.0,
                v_double_right,
                right_bottom,
                cell_width,
                cell_height,
            );
        }
    }

    match segments.right {
        Empty => {}
        Light => push_box_rect_px(
            &mut geometry,
            right_left,
            h_light_top,
            cell_width,
            h_light_bottom,
            cell_width,
            cell_height,
        ),
        Heavy => push_box_rect_px(
            &mut geometry,
            right_left,
            h_heavy_top,
            cell_width,
            h_heavy_bottom,
            cell_width,
            cell_height,
        ),
        Double => {
            let top_left = if segments.up == Double {
                v_light_right
            } else {
                right_left
            };
            let bottom_left = if segments.down == Double {
                v_light_right
            } else {
                right_left
            };
            push_box_rect_px(
                &mut geometry,
                top_left,
                h_double_top,
                cell_width,
                h_light_top,
                cell_width,
                cell_height,
            );
            push_box_rect_px(
                &mut geometry,
                bottom_left,
                h_light_bottom,
                cell_width,
                h_double_bottom,
                cell_width,
                cell_height,
            );
        }
    }

    match segments.down {
        Empty => {}
        Light => push_box_rect_px(
            &mut geometry,
            v_light_left,
            down_top,
            v_light_right,
            cell_height,
            cell_width,
            cell_height,
        ),
        Heavy => push_box_rect_px(
            &mut geometry,
            v_heavy_left,
            down_top,
            v_heavy_right,
            cell_height,
            cell_width,
            cell_height,
        ),
        Double => {
            let left_top = if segments.left == Double {
                h_light_bottom
            } else {
                down_top
            };
            let right_top = if segments.right == Double {
                h_light_bottom
            } else {
                down_top
            };
            push_box_rect_px(
                &mut geometry,
                v_double_left,
                left_top,
                v_light_left,
                cell_height,
                cell_width,
                cell_height,
            );
            push_box_rect_px(
                &mut geometry,
                v_light_right,
                right_top,
                v_double_right,
                cell_height,
                cell_width,
                cell_height,
            );
        }
    }

    match segments.left {
        Empty => {}
        Light => push_box_rect_px(
            &mut geometry,
            0.0,
            h_light_top,
            left_right,
            h_light_bottom,
            cell_width,
            cell_height,
        ),
        Heavy => push_box_rect_px(
            &mut geometry,
            0.0,
            h_heavy_top,
            left_right,
            h_heavy_bottom,
            cell_width,
            cell_height,
        ),
        Double => {
            let top_right = if segments.up == Double {
                v_light_left
            } else {
                left_right
            };
            let bottom_right = if segments.down == Double {
                v_light_left
            } else {
                left_right
            };
            push_box_rect_px(
                &mut geometry,
                0.0,
                h_double_top,
                top_right,
                h_light_top,
                cell_width,
                cell_height,
            );
            push_box_rect_px(
                &mut geometry,
                0.0,
                h_light_bottom,
                bottom_right,
                h_double_bottom,
                cell_width,
                cell_height,
            );
        }
    }

    geometry.merge_collinear_overlaps();

    geometry
}

/// Convenience wrapper: looks up `box_draw_segments` and, if the codepoint is a
/// rectangular connector, converts the descriptor into cell-relative geometry.
///
/// Returns `None` for rounded corners, diagonals, and non-box-drawing characters.
fn box_draw_geometry_for_char(
    c: char,
    cell_width: f32,
    cell_height: f32,
    font_size: f32,
) -> Option<BlockElementGeometry> {
    box_draw_segments(c)
        .map(|segments| box_draw_geometry(segments, cell_width, cell_height, font_size))
}

fn snapped_block_rect_bounds(
    cell_bounds: Bounds<Pixels>,
    rect: BlockRectSpec,
) -> Option<Bounds<Pixels>> {
    let origin_x: f32 = cell_bounds.origin.x.into();
    let origin_y: f32 = cell_bounds.origin.y.into();
    let cell_width: f32 = cell_bounds.size.width.into();
    let cell_height: f32 = cell_bounds.size.height.into();

    let left = (origin_x + cell_width * rect.left).round();
    let right = (origin_x + cell_width * rect.right).round();
    let top = (origin_y + cell_height * rect.top).round();
    let bottom = (origin_y + cell_height * rect.bottom).round();

    let width = right - left;
    let height = bottom - top;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    Some(Bounds {
        origin: point(px(left), px(top)),
        size: Size {
            width: px(width),
            height: px(height),
        },
    })
}

fn snapped_quad_bounds(bounds: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let origin_x: f32 = bounds.origin.x.into();
    let origin_y: f32 = bounds.origin.y.into();
    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();

    let left = origin_x.round();
    let right = (origin_x + width).round();
    let top = origin_y.round();
    let bottom = (origin_y + height).round();

    let snapped_width = right - left;
    let snapped_height = bottom - top;
    if snapped_width <= 0.0 || snapped_height <= 0.0 {
        return None;
    }

    Some(Bounds {
        origin: point(px(left), px(top)),
        size: Size {
            width: px(snapped_width),
            height: px(snapped_height),
        },
    })
}

fn should_paint_clear_bg(color: Hsla) -> bool {
    color.a > f32::EPSILON
}

fn paint_block_element_quad(
    window: &mut Window,
    cell_bounds: Bounds<Pixels>,
    geometry: BlockElementGeometry,
    color: Hsla,
) {
    for rect in geometry.rects() {
        if let Some(bounds) = snapped_block_rect_bounds(cell_bounds, *rect) {
            let mut fill = color;
            fill.a *= rect.alpha;
            window.paint_quad(quad(
                bounds,
                px(0.0),
                fill,
                gpui::Edges::default(),
                Hsla::transparent_black(),
                gpui::BorderStyle::default(),
            ));
        }
    }
}

fn paint_sextant_mosaic(window: &mut Window, cell_bounds: Bounds<Pixels>, packed: u8, color: Hsla) {
    let cell_origin_x: f32 = cell_bounds.origin.x.into();
    let cell_origin_y: f32 = cell_bounds.origin.y.into();
    let cell_w: f32 = cell_bounds.size.width.into();
    let cell_h: f32 = cell_bounds.size.height.into();

    for row in 0..3usize {
        for col in 0..2usize {
            let bit = 5usize - (row * 2 + col);
            if (packed & (1 << bit)) != 0 {
                continue;
            }

            let left = (cell_origin_x + cell_w * (col as f32 / 2.0)).floor();
            let right = (cell_origin_x + cell_w * ((col + 1) as f32 / 2.0)).ceil();
            let top = (cell_origin_y + cell_h * (row as f32 / 3.0)).floor();
            let bottom = (cell_origin_y + cell_h * ((row + 1) as f32 / 3.0)).ceil();

            let width = right - left;
            let height = bottom - top;
            if width <= 0.0 || height <= 0.0 {
                continue;
            }

            let bounds = Bounds {
                origin: point(px(left), px(top)),
                size: Size {
                    width: px(width),
                    height: px(height),
                },
            };
            if let Some(bounds) = snapped_quad_bounds(bounds) {
                window.paint_quad(quad(
                    bounds,
                    px(0.0),
                    color,
                    gpui::Edges::default(),
                    Hsla::transparent_black(),
                    gpui::BorderStyle::default(),
                ));
            }
        }
    }
}

fn rounded_corner_char(c: char) -> bool {
    matches!(c, '\u{256D}' | '\u{256E}' | '\u{256F}' | '\u{2570}')
}

fn diagonal_char(c: char) -> bool {
    matches!(c, '\u{2571}' | '\u{2572}' | '\u{2573}')
}

/// Resolved path geometry for a rounded-corner box-drawing glyph.
///
/// The path is: `start` → straight to `curve_start` → cubic Bézier
/// (`control_a`, `control_b`) → `curve_end` → straight to `end`. This gives
/// a short stub on each cell edge that aligns with adjacent straight box lines,
/// connected by a quarter-circle arc in the cell interior.
#[derive(Clone, Copy, Debug)]
struct RoundedCornerPathSpec {
    start: gpui::Point<Pixels>,
    curve_start: gpui::Point<Pixels>,
    control_a: gpui::Point<Pixels>,
    control_b: gpui::Point<Pixels>,
    curve_end: gpui::Point<Pixels>,
    end: gpui::Point<Pixels>,
    stroke_width: Pixels,
}

/// Resolved path geometry for a diagonal box-drawing glyph.
///
/// A single line segment from `start` to `end`. Both endpoints intentionally
/// overshoot the cell boundary by a slope-dependent amount so that adjacent
/// diagonal cells join seamlessly without pixel gaps.
#[derive(Clone, Copy, Debug)]
struct DiagonalPathSpec {
    start: gpui::Point<Pixels>,
    end: gpui::Point<Pixels>,
    stroke_width: Pixels,
}

/// Computes the midpoint of a stroke that is pixel-snapped to integer boundaries.
///
/// Rounds both edges of the stroke independently, then returns their average.
/// This prevents sub-pixel shimmer on odd-width strokes across HiDPI scales.
fn snapped_stroke_center(origin: Pixels, size: Pixels, stroke_width: Pixels) -> Pixels {
    let origin_px: f32 = origin.into();
    let size_px: f32 = size.into();
    let stroke_px: f32 = stroke_width.into();
    let center_px = origin_px + size_px / 2.0;
    let min_px = (center_px - stroke_px / 2.0).round();
    let max_px = (center_px + stroke_px / 2.0).round();
    px((min_px + max_px) / 2.0)
}

// Rounded box-drawing corners use short straight stubs plus a cubic arc so
// they meet adjacent edge-aligned box lines without visible seams.
fn rounded_corner_path_spec(
    cell_bounds: Bounds<Pixels>,
    glyph: char,
    stroke_width: Pixels,
) -> Option<RoundedCornerPathSpec> {
    let cell_bounds = snapped_quad_bounds(cell_bounds)?;
    let origin = cell_bounds.origin;
    let width = cell_bounds.size.width;
    let height = cell_bounds.size.height;
    let width_px: f32 = width.into();
    let height_px: f32 = height.into();
    let stroke_px: f32 = stroke_width.into();
    let radius = px(((width_px.min(height_px) - stroke_px).max(0.0)) / 2.0);
    let ctrl_offset = radius / 4.0;
    let center_x = snapped_stroke_center(origin.x, width, stroke_width);
    let center_y = snapped_stroke_center(origin.y, height, stroke_width);
    let left_center = point(origin.x, center_y);
    let right_center = point(origin.x + width, center_y);
    let top_center = point(center_x, origin.y);
    let bottom_center = point(center_x, origin.y + height);

    match glyph {
        '\u{256D}' => Some(RoundedCornerPathSpec {
            start: bottom_center,
            curve_start: point(center_x, center_y + radius),
            control_a: point(center_x, center_y + ctrl_offset),
            control_b: point(center_x + ctrl_offset, center_y),
            curve_end: point(center_x + radius, center_y),
            end: right_center,
            stroke_width,
        }),
        '\u{256E}' => Some(RoundedCornerPathSpec {
            start: bottom_center,
            curve_start: point(center_x, center_y + radius),
            control_a: point(center_x, center_y + ctrl_offset),
            control_b: point(center_x - ctrl_offset, center_y),
            curve_end: point(center_x - radius, center_y),
            end: left_center,
            stroke_width,
        }),
        '\u{256F}' => Some(RoundedCornerPathSpec {
            start: top_center,
            curve_start: point(center_x, center_y - radius),
            control_a: point(center_x, center_y - ctrl_offset),
            control_b: point(center_x - ctrl_offset, center_y),
            curve_end: point(center_x - radius, center_y),
            end: left_center,
            stroke_width,
        }),
        '\u{2570}' => Some(RoundedCornerPathSpec {
            start: top_center,
            curve_start: point(center_x, center_y - radius),
            control_a: point(center_x, center_y - ctrl_offset),
            control_b: point(center_x + ctrl_offset, center_y),
            curve_end: point(center_x + radius, center_y),
            end: right_center,
            stroke_width,
        }),
        _ => None,
    }
}

fn paint_rounded_corner_path(
    window: &mut Window,
    cell_bounds: Bounds<Pixels>,
    glyph: char,
    color: Hsla,
    font_size: Pixels,
) {
    let stroke_width = px((Into::<f32>::into(font_size) * 0.0675).ceil().max(1.0));
    let Some(spec) = rounded_corner_path_spec(cell_bounds, glyph, stroke_width) else {
        return;
    };

    let mut builder = PathBuilder::stroke(spec.stroke_width);
    builder.move_to(spec.start);
    builder.line_to(spec.curve_start);
    builder.cubic_bezier_to(spec.curve_end, spec.control_a, spec.control_b);
    builder.line_to(spec.end);

    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn diagonal_path_specs(
    cell_bounds: Bounds<Pixels>,
    glyph: char,
    stroke_width: Pixels,
) -> Option<(DiagonalPathSpec, Option<DiagonalPathSpec>)> {
    let cell_bounds = snapped_quad_bounds(cell_bounds)?;
    let origin = cell_bounds.origin;
    let width = cell_bounds.size.width;
    let height = cell_bounds.size.height;
    let width_px: f32 = width.into();
    let height_px: f32 = height.into();
    if width_px <= 0.0 || height_px <= 0.0 {
        return None;
    }

    let slope_x = px(0.5 * (width_px / height_px).min(1.0));
    let slope_y = px(0.5 * (height_px / width_px).min(1.0));

    let upper_right_to_lower_left = DiagonalPathSpec {
        start: point(origin.x + width + slope_x, origin.y - slope_y),
        end: point(origin.x - slope_x, origin.y + height + slope_y),
        stroke_width,
    };
    let upper_left_to_lower_right = DiagonalPathSpec {
        start: point(origin.x - slope_x, origin.y - slope_y),
        end: point(origin.x + width + slope_x, origin.y + height + slope_y),
        stroke_width,
    };

    match glyph {
        '\u{2571}' => Some((upper_right_to_lower_left, None)),
        '\u{2572}' => Some((upper_left_to_lower_right, None)),
        '\u{2573}' => Some((upper_right_to_lower_left, Some(upper_left_to_lower_right))),
        _ => None,
    }
}

fn paint_diagonal_path(
    window: &mut Window,
    cell_bounds: Bounds<Pixels>,
    glyph: char,
    color: Hsla,
    font_size: Pixels,
) {
    let stroke_width = px((Into::<f32>::into(font_size) * 0.0675).ceil().max(1.0));
    let Some((primary, secondary)) = diagonal_path_specs(cell_bounds, glyph, stroke_width) else {
        return;
    };

    for spec in [Some(primary), secondary].into_iter().flatten() {
        let mut builder = PathBuilder::stroke(spec.stroke_width);
        builder.move_to(spec.start);
        builder.line_to(spec.end);

        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }
}

fn hsla_bits(color: Hsla) -> [u32; 4] {
    [
        color.h.to_bits(),
        color.s.to_bits(),
        color.l.to_bits(),
        color.a.to_bits(),
    ]
}

fn push_row_if_in_bounds(rows: &mut Vec<usize>, maybe_row: Option<usize>, row_count: usize) {
    if let Some(row) = maybe_row
        && row < row_count
    {
        rows.push(row);
    }
}

fn sorted_dedup_rows(rows: &mut Vec<usize>) {
    rows.sort_unstable();
    rows.dedup();
}

fn text_batches_match_without_row(lhs: &TextBatch, rhs: &TextBatch) -> bool {
    lhs.start_col == rhs.start_col
        && lhs.text == rhs.text
        && lhs.bold == rhs.bold
        && lhs.fg == rhs.fg
        && lhs.underline == rhs.underline
        && lhs.cell_len == rhs.cell_len
}

fn block_draws_match_without_row(lhs: &BlockDraw, rhs: &BlockDraw) -> bool {
    lhs.col == rhs.col && lhs.geometry == rhs.geometry && lhs.fg == rhs.fg
}

fn rounded_corner_draws_match_without_row(
    lhs: &RoundedCornerDraw,
    rhs: &RoundedCornerDraw,
) -> bool {
    lhs.col == rhs.col && lhs.glyph == rhs.glyph && lhs.fg == rhs.fg
}

fn diagonal_draws_match_without_row(lhs: &DiagonalDraw, rhs: &DiagonalDraw) -> bool {
    lhs.col == rhs.col && lhs.glyph == rhs.glyph && lhs.fg == rhs.fg
}

fn sextant_draws_match_without_row(lhs: &SextantDraw, rhs: &SextantDraw) -> bool {
    lhs.col == rhs.col && lhs.packed == rhs.packed && lhs.fg == rhs.fg
}

/// Returns the inclusive column range `(start, end)` covered by a draw op.
fn draw_op_col_range(op: &TextDrawOp) -> (usize, usize) {
    match op {
        TextDrawOp::Batch(batch) => {
            let end = if batch.cell_len == 0 {
                batch.start_col
            } else {
                batch.start_col + batch.cell_len - 1
            };
            (batch.start_col, end)
        }
        TextDrawOp::Block(block) => (block.col, block.col),
        TextDrawOp::Sextant(sextant) => (sextant.col, sextant.col),
        TextDrawOp::RoundedCorner(corner) => (corner.col, corner.col),
        TextDrawOp::Diagonal(diagonal) => (diagonal.col, diagonal.col),
    }
}

/// Returns `true` if two inclusive column ranges overlap.
fn col_ranges_overlap(a: (usize, usize), b: (usize, usize)) -> bool {
    let start_a = a.0.min(a.1);
    let end_a = a.0.max(a.1);
    let start_b = b.0.min(b.1);
    let end_b = b.0.max(b.1);

    start_a <= end_b && start_b <= end_a
}

fn text_draw_ops_match_without_row(lhs: &TextDrawOp, rhs: &TextDrawOp) -> bool {
    match (lhs, rhs) {
        (TextDrawOp::Batch(lhs), TextDrawOp::Batch(rhs)) => {
            text_batches_match_without_row(lhs, rhs)
        }
        (TextDrawOp::Block(lhs), TextDrawOp::Block(rhs)) => block_draws_match_without_row(lhs, rhs),
        (TextDrawOp::Sextant(lhs), TextDrawOp::Sextant(rhs)) => {
            sextant_draws_match_without_row(lhs, rhs)
        }
        (TextDrawOp::RoundedCorner(lhs), TextDrawOp::RoundedCorner(rhs)) => {
            rounded_corner_draws_match_without_row(lhs, rhs)
        }
        (TextDrawOp::Diagonal(lhs), TextDrawOp::Diagonal(rhs)) => {
            diagonal_draws_match_without_row(lhs, rhs)
        }
        _ => false,
    }
}

fn cached_row_draw_ops_match_without_row(lhs: &CachedRowPaintOps, rhs: &CachedRowPaintOps) -> bool {
    lhs.background_spans == rhs.background_spans
        && lhs.draw_ops.len() == rhs.draw_ops.len()
        && lhs
            .draw_ops
            .iter()
            .zip(rhs.draw_ops.iter())
            .all(|(lhs, rhs)| text_draw_ops_match_without_row(lhs, rhs))
}

fn find_matching_previous_row_ops_index(
    row: usize,
    row_ops: &CachedRowPaintOps,
    previous_row_ops: &[CachedRowPaintOps],
) -> Option<usize> {
    for preferred in [Some(row), row.checked_add(1), row.checked_sub(1)] {
        let Some(index) = preferred else {
            continue;
        };
        let Some(previous) = previous_row_ops.get(index) else {
            continue;
        };
        if cached_row_draw_ops_match_without_row(row_ops, previous) {
            return Some(index);
        }
    }

    previous_row_ops.iter().enumerate().find_map(|(index, previous)| {
        matches!(index, i if i != row && Some(i) != row.checked_add(1) && Some(i) != row.checked_sub(1))
            .then_some(previous)
            .filter(|previous| cached_row_draw_ops_match_without_row(row_ops, previous))
            .map(|_| index)
    })
}

enum PreviousRowOps {
    Full(Vec<CachedRowPaintOps>),
    Partial(Vec<(usize, CachedRowPaintOps)>),
}

impl PreviousRowOps {
    fn get(&self, row: usize) -> Option<&CachedRowPaintOps> {
        match self {
            Self::Full(rows) => rows.get(row),
            Self::Partial(rows) => rows
                .iter()
                .find_map(|(previous_row, ops)| (*previous_row == row).then_some(ops)),
        }
    }

    fn find_matching_index(&self, row: usize, row_ops: &CachedRowPaintOps) -> Option<usize> {
        match self {
            Self::Full(rows) => find_matching_previous_row_ops_index(row, row_ops, rows),
            Self::Partial(rows) => rows.iter().find_map(|(previous_row, previous_ops)| {
                cached_row_draw_ops_match_without_row(row_ops, previous_ops)
                    .then_some(*previous_row)
            }),
        }
    }
}

impl Element for TerminalGrid {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let width = self.cell_size.width * self.cols as f32;
        let height = self.cell_size.height * self.rows as f32;

        let layout_id = window.request_layout(
            gpui::Style {
                size: gpui::Size {
                    width: gpui::Length::Definite(gpui::DefiniteLength::Absolute(
                        gpui::AbsoluteLength::Pixels(width),
                    )),
                    height: gpui::Length::Definite(gpui::DefiniteLength::Absolute(
                        gpui::AbsoluteLength::Pixels(height),
                    )),
                },
                ..Default::default()
            },
            [],
            cx,
        );

        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        increment_grid_paint_count();
        let t_paint = terminal_ui_render_metrics_enabled().then(Instant::now);
        self.paint_with_row_cache(bounds, window, cx);
        if let Some(t_paint) = t_paint {
            add_span_grid_paint_us(t_paint.elapsed().as_micros() as u64);
        }
    }
}

impl TerminalGrid {
    fn paint_style_key(&self) -> GridPaintStyleKey {
        GridPaintStyleKey {
            cols: self.cols,
            rows: self.rows,
            cell_width_bits: Into::<f32>::into(self.cell_size.width).to_bits(),
            cell_height_bits: Into::<f32>::into(self.cell_size.height).to_bits(),
            clear_bg: hsla_bits(self.clear_bg),
            terminal_surface_bg: hsla_bits(self.terminal_surface_bg),
            selection_bg: hsla_bits(self.selection_bg),
            selection_fg: hsla_bits(self.selection_fg),
            search_match_bg: hsla_bits(self.search_match_bg),
            search_current_bg: hsla_bits(self.search_current_bg),
            cursor_style: self.cursor_style,
            font_family: self.font_family.clone(),
            font_size_bits: Into::<f32>::into(self.font_size).to_bits(),
        }
    }

    fn row_background_fill(&self, cell: &CellRenderInfo) -> Option<Hsla> {
        if cell.selected {
            Some(self.selection_bg)
        } else if cell.search_current {
            Some(self.search_current_bg)
        } else if cell.search_match {
            Some(self.search_match_bg)
        } else if cell.bg.a <= 0.01 {
            None
        } else if cell.uses_terminal_default_bg {
            (cell.bg != self.terminal_surface_bg).then_some(cell.bg)
        } else {
            Some(cell.bg)
        }
    }

    fn build_row_background_spans_into(
        &self,
        row_cells: &[CellRenderInfo],
        color_cache: &mut HashMap<[u32; 4], Option<Hsla>>,
        spans: &mut Vec<BackgroundSpan>,
    ) {
        spans.clear();
        if row_cells.is_empty() {
            return;
        }
        let mut current: Option<BackgroundSpan> = None;

        for cell in row_cells {
            // For cells with default background that aren't highlighted, cache the fill
            // resolution to avoid repeated float comparisons against terminal_surface_bg.
            let fill = if !cell.selected
                && !cell.search_current
                && !cell.search_match
                && cell.bg.a > 0.01
                && cell.uses_terminal_default_bg
            {
                let key = hsla_bits(cell.bg);
                *color_cache
                    .entry(key)
                    .or_insert_with(|| (cell.bg != self.terminal_surface_bg).then_some(cell.bg))
            } else {
                self.row_background_fill(cell)
            };
            match (current.as_mut(), fill) {
                (Some(span), Some(color))
                    if span.color == color && span.end_col_exclusive == cell.col =>
                {
                    span.end_col_exclusive = cell.col.saturating_add(1);
                }
                (Some(span), Some(color)) => {
                    spans.push(*span);
                    current = Some(BackgroundSpan {
                        start_col: cell.col,
                        end_col_exclusive: cell.col.saturating_add(1),
                        color,
                    });
                }
                (Some(span), None) => {
                    spans.push(*span);
                    current = None;
                }
                (None, Some(color)) => {
                    current = Some(BackgroundSpan {
                        start_col: cell.col,
                        end_col_exclusive: cell.col.saturating_add(1),
                        color,
                    });
                }
                (None, None) => {}
            }
        }

        if let Some(span) = current {
            spans.push(span);
        }
    }

    fn collect_row_draw_ops_into(
        &self,
        row_cells: &[CellRenderInfo],
        cursor_fg: Hsla,
        highlight_fg: Hsla,
        ops: &mut Vec<TextDrawOp>,
    ) {
        ops.clear();
        let mut current: Option<TextBatchBuilder> = None;
        let cell_w: f32 = self.cell_size.width.into();
        let cell_h: f32 = self.cell_size.height.into();
        let font_sz: f32 = self.font_size.into();

        for (index, cell) in row_cells.iter().enumerate() {
            if !Self::cell_is_drawable_text(cell) {
                Self::push_pending_text_batch(&mut current, ops);
                continue;
            }

            let fg = self.cell_fg_color(cell, cursor_fg, highlight_fg);
            if rounded_corner_char(cell.char) {
                Self::push_pending_text_batch(&mut current, ops);
                ops.push(TextDrawOp::RoundedCorner(RoundedCornerDraw {
                    row: cell.row,
                    col: cell.col,
                    glyph: cell.char,
                    fg,
                }));
                continue;
            }

            if diagonal_char(cell.char) {
                Self::push_pending_text_batch(&mut current, ops);
                ops.push(TextDrawOp::Diagonal(DiagonalDraw {
                    row: cell.row,
                    col: cell.col,
                    glyph: cell.char,
                    fg,
                }));
                continue;
            }

            if let Some(packed) = sextant_char_to_packed(cell.char) {
                Self::push_pending_text_batch(&mut current, ops);
                ops.push(TextDrawOp::Sextant(SextantDraw {
                    row: cell.row,
                    col: cell.col,
                    packed,
                    fg,
                }));
                continue;
            }

            if let Some(geometry) = block_element_geometry(cell.char)
                .or_else(|| {
                    should_render_braille_as_geometry(row_cells, index)
                        .then(|| braille_geometry(cell.char))
                        .flatten()
                })
                .or_else(|| box_draw_geometry_for_char(cell.char, cell_w, cell_h, font_sz))
            {
                Self::push_pending_text_batch(&mut current, ops);
                ops.push(TextDrawOp::Block(BlockDraw {
                    row: cell.row,
                    col: cell.col,
                    geometry,
                    fg,
                }));
                continue;
            }

            let underline = self.cell_underline(cell.row, cell.col, fg);
            let key = TextBatchKey {
                bold: cell.bold,
                fg,
            };

            let should_append = current
                .as_ref()
                .is_some_and(|batch| batch.can_append(cell.col, cell.row, key, &underline));
            if should_append {
                if let Some(batch) = current.as_mut() {
                    batch.append_char(cell.char);
                }
                continue;
            }

            Self::push_pending_text_batch(&mut current, ops);
            current = Some(TextBatchBuilder::new(
                cell.col, cell.row, cell.char, key, underline,
            ));
        }

        Self::push_pending_text_batch(&mut current, ops);
    }

    #[cfg(test)]
    fn rebuild_cached_row_ops_into(
        &self,
        row_cells: &[CellRenderInfo],
        cursor_fg: Hsla,
        highlight_fg: Hsla,
        color_cache: &mut HashMap<[u32; 4], Option<Hsla>>,
        scratch_bg: &mut Vec<BackgroundSpan>,
        scratch_ops: &mut Vec<TextDrawOp>,
    ) -> CachedRowPaintOps {
        self.collect_row_draw_ops_into(row_cells, cursor_fg, highlight_fg, scratch_ops);
        self.build_row_background_spans_into(row_cells, color_cache, scratch_bg);
        let ops_len = scratch_ops.len();
        let bg_cap = scratch_bg.capacity();
        let ops_cap = scratch_ops.capacity();
        CachedRowPaintOps {
            background_spans: std::mem::replace(scratch_bg, Vec::with_capacity(bg_cap)),
            draw_ops: std::mem::replace(scratch_ops, Vec::with_capacity(ops_cap)),
            shaped_lines: vec![None; ops_len],
        }
    }

    /// Convenience wrapper for tests — allocates fresh scratch buffers per call.
    #[cfg(test)]
    fn rebuild_cached_row_ops(
        &self,
        row_cells: &[CellRenderInfo],
        cursor_fg: Hsla,
        highlight_fg: Hsla,
        color_cache: &mut HashMap<[u32; 4], Option<Hsla>>,
    ) -> CachedRowPaintOps {
        let mut scratch_bg = Vec::new();
        let mut scratch_ops = Vec::new();
        self.rebuild_cached_row_ops_into(
            row_cells,
            cursor_fg,
            highlight_fg,
            color_cache,
            &mut scratch_bg,
            &mut scratch_ops,
        )
    }

    fn clear_bounds(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        if !should_paint_clear_bg(self.clear_bg) {
            return;
        }
        window.paint_quad(quad(
            bounds,
            px(0.0),
            self.clear_bg,
            gpui::Edges::default(),
            Hsla::transparent_black(),
            gpui::BorderStyle::default(),
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_cached_row_ops(
        &self,
        row: usize,
        row_ops: &mut CachedRowPaintOps,
        origin: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
        font_normal: &Font,
        font_bold: &Font,
    ) {
        for span in &row_ops.background_spans {
            if span.start_col >= span.end_col_exclusive {
                continue;
            }
            let x = origin.x + self.cell_size.width * span.start_col as f32;
            let width_cells = span.end_col_exclusive.saturating_sub(span.start_col);
            if width_cells == 0 {
                continue;
            }
            let cell_bounds = Bounds {
                origin: point(x, origin.y),
                size: Size {
                    width: self.cell_size.width * width_cells as f32,
                    height: self.cell_size.height,
                },
            };
            if let Some(bounds) = snapped_quad_bounds(cell_bounds) {
                window.paint_quad(quad(
                    bounds,
                    px(0.0),
                    span.color,
                    gpui::Edges::default(),
                    Hsla::transparent_black(),
                    gpui::BorderStyle::default(),
                ));
            }
        }

        // Keep block cursors beneath glyphs, but paint line cursors on top so text/block ops
        // cannot overdraw the line.
        if self.cursor_style == TerminalCursorStyle::Block {
            self.paint_cursor_for_row(row, origin, window);
        }

        for (index, op) in row_ops.draw_ops.iter().enumerate() {
            match op {
                TextDrawOp::Batch(batch) => {
                    let x = origin.x + self.cell_size.width * batch.start_col as f32;
                    let line = if row_ops.shaped_lines.get(index).is_some_and(Option::is_some) {
                        increment_shaped_line_cache_hit();
                        row_ops.shaped_lines[index]
                            .as_ref()
                            .expect("cached shaped line must exist")
                    } else {
                        increment_shaped_line_cache_miss();
                        increment_shape_line_calls();
                        let font = if batch.bold { font_bold } else { font_normal };
                        let run = TextRun {
                            len: batch.text.len(),
                            font: font.clone(),
                            color: batch.fg,
                            background_color: None,
                            underline: batch.underline,
                            strikethrough: None,
                        };
                        let t_shape = terminal_ui_render_metrics_enabled().then(Instant::now);
                        row_ops.shaped_lines[index] =
                            Some(Rc::new(window.text_system().shape_line(
                                batch.text.clone(),
                                self.font_size,
                                &[run],
                                Some(self.cell_size.width),
                            )));
                        if let Some(t_shape) = t_shape {
                            add_span_text_shaping_us(t_shape.elapsed().as_micros() as u64);
                        }
                        row_ops.shaped_lines[index]
                            .as_ref()
                            .expect("cached shaped line must be created")
                    };
                    let _ = line.paint(
                        point(x, origin.y),
                        self.cell_size.height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
                TextDrawOp::Block(block) => {
                    let x = origin.x + self.cell_size.width * block.col as f32;
                    let cell_bounds = Bounds {
                        origin: point(x, origin.y),
                        size: self.cell_size,
                    };
                    paint_block_element_quad(window, cell_bounds, block.geometry, block.fg);
                }
                TextDrawOp::Sextant(sextant) => {
                    let x = origin.x + self.cell_size.width * sextant.col as f32;
                    let cell_bounds = Bounds {
                        origin: point(x, origin.y),
                        size: self.cell_size,
                    };
                    paint_sextant_mosaic(window, cell_bounds, sextant.packed, sextant.fg);
                }
                TextDrawOp::RoundedCorner(corner) => {
                    let x = origin.x + self.cell_size.width * corner.col as f32;
                    let cell_bounds = Bounds {
                        origin: point(x, origin.y),
                        size: self.cell_size,
                    };
                    paint_rounded_corner_path(
                        window,
                        cell_bounds,
                        corner.glyph,
                        corner.fg,
                        self.font_size,
                    );
                }
                TextDrawOp::Diagonal(diagonal) => {
                    let x = origin.x + self.cell_size.width * diagonal.col as f32;
                    let cell_bounds = Bounds {
                        origin: point(x, origin.y),
                        size: self.cell_size,
                    };
                    paint_diagonal_path(
                        window,
                        cell_bounds,
                        diagonal.glyph,
                        diagonal.fg,
                        self.font_size,
                    );
                }
            }
        }

        if self.cursor_style == TerminalCursorStyle::Line {
            self.paint_cursor_for_row(row, origin, window);
        }
    }

    fn dirty_rows_for_pass(&self, cache: &mut TerminalGridPaintCache) -> (bool, bool, Vec<usize>) {
        let style_key = self.paint_style_key();
        let style_changed = cache.style_key.as_ref() != Some(&style_key);
        if style_changed {
            cache.color_cache.clear();
        }
        cache.style_key = Some(style_key);

        let mut full_repaint =
            style_changed || matches!(self.paint_damage, TerminalGridPaintDamage::Full);
        let mut rows = std::mem::take(&mut cache.dirty_rows);
        rows.clear();
        if let TerminalGridPaintDamage::Rows(damaged_rows) = &self.paint_damage {
            rows.extend(damaged_rows.iter().copied().filter(|row| *row < self.rows));
        }
        if let TerminalGridPaintDamage::RowRanges(spans) = &self.paint_damage {
            for &(row, left, right) in spans.iter() {
                if row < self.rows {
                    rows.push(row);
                    // Merge multiple spans on the same row into one union range
                    cache.dirty_col_ranges[row] = Some(match cache.dirty_col_ranges[row] {
                        None => (left, right),
                        Some((prev_l, prev_r)) => (prev_l.min(left), prev_r.max(right)),
                    });
                }
            }
        }

        if cache.last_cursor_cell != self.cursor_cell {
            push_row_if_in_bounds(
                &mut rows,
                cache.last_cursor_cell.map(|(_, row)| row),
                self.rows,
            );
            push_row_if_in_bounds(&mut rows, self.cursor_cell.map(|(_, row)| row), self.rows);
        }

        // Blink visibility changed → only need to rebuild for Block cursor, since the
        // cursor cell's text fg color is baked into draw ops. Line cursor is a plain
        // quad painted after row ops and needs no row rebuild on blink.
        if cache.last_cursor_visible != self.cursor_visible
            && self.cursor_style == TerminalCursorStyle::Block
        {
            push_row_if_in_bounds(&mut rows, self.cursor_cell.map(|(_, row)| row), self.rows);
        }

        if cache.last_hovered_link_range != self.hovered_link_range {
            push_row_if_in_bounds(
                &mut rows,
                cache.last_hovered_link_range.map(|(row, _, _)| row),
                self.rows,
            );
            push_row_if_in_bounds(
                &mut rows,
                self.hovered_link_range.map(|(row, _, _)| row),
                self.rows,
            );
        }

        if self.rows == 0 || self.cols == 0 {
            rows.clear();
            full_repaint = false;
        }

        cache.last_cursor_cell = self.cursor_cell;
        cache.last_cursor_visible = self.cursor_visible;
        cache.last_hovered_link_range = self.hovered_link_range;
        sorted_dedup_rows(&mut rows);

        (full_repaint, style_changed, rows)
    }

    fn paint_cursor_for_row(&self, row: usize, origin: gpui::Point<Pixels>, window: &mut Window) {
        let Some((cursor_col, cursor_row)) = self.cursor_cell else {
            return;
        };
        if !self.cursor_visible {
            return;
        }
        if cursor_row != row {
            return;
        }
        let x = origin.x + self.cell_size.width * cursor_col as f32;
        let y = origin.y;
        let cell_bounds = Bounds {
            origin: point(x, y),
            size: self.cell_size,
        };
        let cursor_bounds = match self.cursor_style {
            TerminalCursorStyle::Block => cell_bounds,
            TerminalCursorStyle::Line => {
                let cell_width: f32 = self.cell_size.width.into();
                let cursor_width = px(cell_width.clamp(1.0, 2.0));
                Bounds::new(
                    cell_bounds.origin,
                    Size {
                        width: cursor_width,
                        height: cell_bounds.size.height,
                    },
                )
            }
        };

        window.paint_quad(quad(
            cursor_bounds,
            px(0.0),
            self.cursor_color,
            gpui::Edges::default(),
            Hsla::transparent_black(),
            gpui::BorderStyle::default(),
        ));
    }

    fn rebuild_cached_rows_for_pass(
        &self,
        cache: &mut TerminalGridPaintCache,
        full_repaint: bool,
        style_changed: bool,
        dirty_rows: &[usize],
        cursor_fg: Hsla,
        highlight_fg: Hsla,
    ) {
        // Build a snapshot of previous row ops for ShapedLine reuse, without
        // deep-cloning. For full repaints every slot will be rebuilt, so we swap
        // the entire vec with defaults. For partial repaints we only take the
        // dirty-row entries out of the cache so non-dirty rows keep their
        // existing cached ops (GPUI clears pixels each frame and repaints every
        // row from cache.row_ops, so wiping non-dirty rows would blank them).
        let previous_row_ops = if !style_changed && !cache.row_ops.is_empty() {
            if full_repaint {
                let replacement = vec![CachedRowPaintOps::default(); self.rows];
                Some(PreviousRowOps::Full(std::mem::replace(
                    &mut cache.row_ops,
                    replacement,
                )))
            } else {
                let len = cache.row_ops.len();
                let mut previous = Vec::with_capacity(dirty_rows.len());
                for &row in dirty_rows {
                    if row < len {
                        previous.push((row, std::mem::take(&mut cache.row_ops[row])));
                    }
                }
                Some(PreviousRowOps::Partial(previous))
            }
        } else {
            None
        };
        // Build ops directly into row slots, avoiding per-row temporary allocations.
        let mut rebuild_row = |row: usize| {
            if row >= self.rows {
                return;
            }
            // Read col range hint (Copy) before any mutable borrows.
            let dirty_col_range = cache.dirty_col_ranges.get(row).copied().flatten();

            let Some(row_slot) = cache.row_ops.get_mut(row) else {
                return;
            };

            if let Some(row_cells) = self.cells.get(row) {
                row_slot.draw_ops.clear();
                row_slot.background_spans.clear();
                self.collect_row_draw_ops_into(
                    row_cells.as_slice(),
                    cursor_fg,
                    highlight_fg,
                    &mut row_slot.draw_ops,
                );
                self.build_row_background_spans_into(
                    row_cells.as_slice(),
                    &mut cache.color_cache,
                    &mut row_slot.background_spans,
                );
                row_slot.shaped_lines.clear();
                row_slot.shaped_lines.resize(row_slot.draw_ops.len(), None);
            } else {
                *row_slot = CachedRowPaintOps::default();
                return;
            }

            // 1. Try whole-row ShapedLine reuse: if the entire row's ops match a previous
            //    row, reuse all its ShapedLine objects (existing logic).
            let mut whole_row_reused = false;
            if let Some(previous_row_ops) = previous_row_ops.as_ref()
                && let Some(previous_index) = previous_row_ops.find_matching_index(row, row_slot)
                && let Some(previous) = previous_row_ops.get(previous_index)
                && previous.shaped_lines.len() == row_slot.shaped_lines.len()
            {
                row_slot.shaped_lines.clone_from(&previous.shaped_lines);
                whole_row_reused = true;
            }

            // 2. Per-op ShapedLine reuse: if we know the dirty column range (from RowRanges
            //    damage), reuse ShapedLines for text batches that don't overlap the dirty
            //    region. This avoids re-shaping unchanged text runs when only a few columns
            //    changed (e.g. a single character typed at the cursor).
            if !whole_row_reused
                && let Some(dirty_range) = dirty_col_range
                && let Some(prev_row) = previous_row_ops
                    .as_ref()
                    .and_then(|previous| previous.get(row))
            {
                for (i, op) in row_slot.draw_ops.iter().enumerate() {
                    let op_range = draw_op_col_range(op);
                    if !col_ranges_overlap(op_range, dirty_range)
                        && let Some(prev_op) = prev_row.draw_ops.get(i)
                        && text_draw_ops_match_without_row(op, prev_op)
                    {
                        row_slot.shaped_lines[i] = prev_row.shaped_lines[i].clone();
                    }
                }
            }
        };

        let t0 = terminal_ui_render_metrics_enabled().then(Instant::now);
        if full_repaint {
            for row in 0..self.rows {
                rebuild_row(row);
            }
        } else {
            for row in dirty_rows.iter().copied() {
                rebuild_row(row);
            }
        }
        if let Some(t0) = t0 {
            add_span_row_ops_rebuild_us(t0.elapsed().as_micros() as u64);
        }
    }

    fn paint_with_row_cache(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let origin = bounds.origin;

        let mut cache = self.paint_cache.0.borrow_mut();
        cache.ensure_row_capacity(self.rows);
        let (full_repaint, style_changed, dirty_rows) = self.dirty_rows_for_pass(&mut cache);

        // Rebuild cached Font objects only when the style (font family) changes.
        if style_changed || cache.cached_font_normal.is_none() {
            let terminal_font_features = terminal_font_features();
            cache.cached_font_normal = Some(Font {
                family: self.font_family.clone(),
                features: terminal_font_features.clone(),
                weight: FontWeight::NORMAL,
                ..Default::default()
            });
            cache.cached_font_bold = Some(Font {
                family: self.font_family.clone(),
                features: terminal_font_features,
                weight: FontWeight::BOLD,
                ..Default::default()
            });
        }
        let font_normal = cache.cached_font_normal.clone().unwrap();
        let font_bold = cache.cached_font_bold.clone().unwrap();

        let cursor_fg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        };
        let highlight_fg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.08,
            a: 1.0,
        };

        self.rebuild_cached_rows_for_pass(
            &mut cache,
            full_repaint,
            style_changed,
            dirty_rows.as_ref(),
            cursor_fg,
            highlight_fg,
        );
        cache.dirty_rows = dirty_rows;

        // GPUI paint passes do not preserve previous pixels across frames. Always clear and draw
        // all rows; damage only controls which cached row ops are recomputed.
        self.clear_bounds(
            Bounds {
                origin,
                size: bounds.size,
            },
            window,
        );
        for row in 0..self.rows {
            let row_origin = point(origin.x, origin.y + self.cell_size.height * row as f32);
            self.paint_cached_row_ops(
                row,
                &mut cache.row_ops[row],
                row_origin,
                window,
                cx,
                &font_normal,
                &font_bold,
            );
        }

        drop(cache);
    }

    #[cfg(test)]
    fn cell_count(&self) -> usize {
        self.cells.iter().map(|row| row.len()).sum()
    }

    fn cell_is_drawable_text(cell: &CellRenderInfo) -> bool {
        cell.render_text && cell.char != ' ' && cell.char != '\0' && !cell.char.is_control()
    }

    fn cell_fg_color(&self, cell: &CellRenderInfo, cursor_fg: Hsla, highlight_fg: Hsla) -> Hsla {
        if self.cursor_cell == Some((cell.col, cell.row))
            && self.cursor_style == TerminalCursorStyle::Block
            && self.cursor_visible
        {
            cursor_fg
        } else if cell.selected {
            self.selection_fg
        } else if cell.search_current || cell.search_match {
            highlight_fg
        } else {
            cell.fg
        }
    }

    fn cell_underline(&self, row: usize, col: usize, color: Hsla) -> Option<UnderlineStyle> {
        self.hovered_link_range
            .and_then(|(link_row, start_col, end_col)| {
                if row == link_row && col >= start_col && col <= end_col {
                    Some(UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(color),
                        wavy: false,
                    })
                } else {
                    None
                }
            })
    }

    fn push_pending_text_batch(current: &mut Option<TextBatchBuilder>, ops: &mut Vec<TextDrawOp>) {
        if let Some(builder) = current.take() {
            ops.push(TextDrawOp::Batch(builder.finalize()));
        }
    }

    #[cfg(test)]
    fn collect_draw_ops(&self, cursor_fg: Hsla, highlight_fg: Hsla) -> Vec<TextDrawOp> {
        let mut ops = Vec::with_capacity(self.cell_count());
        let mut scratch = Vec::new();
        for row_cells in self.cells.iter() {
            self.collect_row_draw_ops_into(
                row_cells.as_ref(),
                cursor_fg,
                highlight_fg,
                &mut scratch,
            );
            ops.append(&mut scratch);
        }
        ops
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, Size, point, px};

    #[test]
    fn terminal_font_features_disable_all_spacing_affecting_ligatures() {
        let features = terminal_font_features();
        for tag in ["calt", "liga", "clig"] {
            assert_eq!(
                features
                    .tag_value_list()
                    .iter()
                    .find(|(feature, _)| feature == tag)
                    .map(|(_, value)| *value),
                Some(0),
                "{tag} must be disabled for fixed-cell shaping"
            );
        }
    }

    fn assert_f32_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 1e-6,
            "expected {expected}, got {actual}"
        );
    }

    fn test_color(h: f32, s: f32, l: f32) -> Hsla {
        Hsla { h, s, l, a: 1.0 }
    }

    fn test_cell(col: usize, row: usize, c: char) -> CellRenderInfo {
        CellRenderInfo {
            col,
            row,
            char: c,
            fg: test_color(0.4, 0.5, 0.6),
            bg: test_color(0.0, 0.0, 0.0),
            uses_terminal_default_bg: false,
            bold: false,
            render_text: true,
            selected: false,
            search_current: false,
            search_match: false,
        }
    }

    fn test_grid(
        cells: Vec<CellRenderInfo>,
        hovered: Option<(usize, usize, usize)>,
    ) -> TerminalGrid {
        test_grid_rows(vec![cells], hovered)
    }

    fn test_grid_rows(
        rows: Vec<Vec<CellRenderInfo>>,
        hovered: Option<(usize, usize, usize)>,
    ) -> TerminalGrid {
        let row_count = rows.len();
        let col_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        TerminalGrid {
            cells: Arc::new(rows.into_iter().map(Arc::new).collect()),
            paint_cache: TerminalGridPaintCacheHandle::default(),
            paint_damage: TerminalGridPaintDamage::Full,
            cell_size: Size {
                width: px(10.0),
                height: px(20.0),
            },
            cols: col_count,
            rows: row_count,
            clear_bg: Hsla::transparent_black(),
            terminal_surface_bg: test_color(0.0, 0.0, 0.0),
            cursor_color: test_color(0.1, 0.1, 0.1),
            selection_bg: test_color(0.2, 0.2, 0.2),
            selection_fg: test_color(0.3, 0.3, 0.3),
            search_match_bg: test_color(0.4, 0.4, 0.4),
            search_current_bg: test_color(0.5, 0.5, 0.5),
            hovered_link_range: hovered,
            cursor_cell: None,
            cursor_visible: false,
            font_family: SharedString::from("JetBrains Mono"),
            font_size: px(14.0),
            cursor_style: TerminalCursorStyle::Block,
        }
    }

    fn collect_draw_ops(grid: &TerminalGrid) -> Vec<TextDrawOp> {
        grid.collect_draw_ops(
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 1.0,
            },
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.08,
                a: 1.0,
            },
        )
    }

    fn collect_batches(grid: &TerminalGrid) -> Vec<TextBatch> {
        collect_draw_ops(grid)
            .into_iter()
            .filter_map(|op| match op {
                TextDrawOp::Batch(batch) => Some(batch),
                TextDrawOp::Block(_) | TextDrawOp::Sextant(_) => None,
                TextDrawOp::RoundedCorner(_) => None,
                TextDrawOp::Diagonal(_) => None,
            })
            .collect()
    }

    #[test]
    fn block_element_geometry_is_complete_for_unicode_range() {
        for codepoint in BLOCK_ELEMENTS_START..=BLOCK_ELEMENTS_END {
            let glyph = char::from_u32(codepoint).expect("valid block-element codepoint");
            assert!(
                block_element_geometry(glyph).is_some(),
                "missing geometry for U+{codepoint:04X}"
            );
        }
    }

    #[test]
    fn braille_geometry_supports_non_empty_patterns() {
        let geometry = braille_geometry('\u{28FF}').expect("expected braille geometry");
        assert_eq!(geometry.rect_count, 8);
    }

    #[test]
    fn blank_braille_does_not_emit_geometry() {
        assert!(braille_geometry('\u{2800}').is_none());
    }

    #[test]
    fn box_draw_segments_covers_expected_range() {
        for codepoint in BOX_DRAWING_START..=BOX_DRAWING_END {
            let glyph = char::from_u32(codepoint).expect("valid box-drawing codepoint");
            assert!(
                rounded_corner_char(glyph)
                    || diagonal_char(glyph)
                    || box_draw_geometry_for_char(glyph, 10.0, 20.0, 14.0).is_some(),
                "unexpected box-drawing coverage for U+{codepoint:04X}"
            );
        }
    }

    #[test]
    fn upper_half_block_geometry_covers_top_half() {
        let geometry = block_element_geometry('\u{2580}').expect("expected block geometry");
        assert_eq!(geometry.rect_count, 1);
        let rect = geometry.rects()[0];
        assert_eq!(rect.left, 0.0);
        assert_eq!(rect.top, 0.0);
        assert_eq!(rect.right, 1.0);
        assert_eq!(rect.bottom, 0.5);
        assert_eq!(rect.alpha, 1.0);
    }

    #[test]
    fn upper_half_block_bounds_are_pixel_snapped() {
        let geometry = block_element_geometry('\u{2580}').expect("expected block geometry");
        let rect = geometry.rects()[0];
        let cell_bounds = Bounds {
            origin: point(px(12.3), px(40.7)),
            size: Size {
                width: px(17.8),
                height: px(15.2),
            },
        };

        let snapped = snapped_block_rect_bounds(cell_bounds, rect).expect("expected bounds");

        let x: f32 = snapped.origin.x.into();
        let y: f32 = snapped.origin.y.into();
        let width: f32 = snapped.size.width.into();
        let height: f32 = snapped.size.height.into();
        assert_eq!(x.fract(), 0.0);
        assert_eq!(y.fract(), 0.0);
        assert_eq!(width.fract(), 0.0);
        assert_eq!(height.fract(), 0.0);
    }

    #[test]
    fn draw_ops_render_braille_as_block_geometry() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, '\u{28FF}'),
                test_cell(1, 0, '\u{28FF}'),
                test_cell(2, 0, '\u{28FF}'),
                test_cell(3, 0, 'x'),
            ],
            None,
        );

        let ops = collect_draw_ops(&grid);
        assert!(matches!(&ops[0], TextDrawOp::Block(_)));
        assert!(matches!(&ops[1], TextDrawOp::Block(_)));
        assert!(matches!(&ops[2], TextDrawOp::Block(_)));
        assert!(matches!(&ops[3], TextDrawOp::Batch(_)));
    }

    #[test]
    fn draw_ops_keep_two_cell_braille_spinners_as_text() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, '\u{2830}'),
                test_cell(1, 0, '\u{2830}'),
                test_cell(2, 0, 'x'),
            ],
            None,
        );

        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            &ops[0],
            TextDrawOp::Batch(batch) if batch.text.as_ref() == "\u{2830}\u{2830}x"
        ));
    }

    #[test]
    fn quad_bounds_are_pixel_snapped() {
        let bounds = Bounds {
            origin: point(px(3.4), px(7.6)),
            size: Size {
                width: px(9.2),
                height: px(10.3),
            },
        };

        let snapped = snapped_quad_bounds(bounds).expect("expected bounds");
        let x: f32 = snapped.origin.x.into();
        let y: f32 = snapped.origin.y.into();
        let width: f32 = snapped.size.width.into();
        let height: f32 = snapped.size.height.into();
        assert_eq!(x.fract(), 0.0);
        assert_eq!(y.fract(), 0.0);
        assert_eq!(width.fract(), 0.0);
        assert_eq!(height.fract(), 0.0);
    }

    #[test]
    fn transparent_clear_background_skips_clear_quad() {
        assert!(!should_paint_clear_bg(Hsla::transparent_black()));
        assert!(should_paint_clear_bg(test_color(0.1, 0.2, 0.3)));
    }

    #[test]
    fn fast_path_excludes_non_block_glyphs() {
        assert!(block_element_geometry('\u{2579}').is_none());
        assert!(block_element_geometry('A').is_none());
    }

    #[test]
    fn box_draw_light_horizontal_geometry() {
        let geometry =
            box_draw_geometry_for_char('\u{2500}', 10.0, 20.0, 14.0).expect("expected geometry");

        assert_eq!(geometry.rect_count, 1);
        let rect = geometry.rects()[0];
        assert_f32_eq(rect.left, 0.0);
        assert_f32_eq(rect.right, 1.0);
        assert_f32_eq(rect.top, 0.475);
        assert_f32_eq(rect.bottom, 0.525);
        assert_eq!(rect.alpha, 1.0);
    }

    #[test]
    fn box_draw_light_cross_geometry() {
        let geometry =
            box_draw_geometry_for_char('\u{253C}', 10.0, 20.0, 14.0).expect("expected geometry");

        assert_eq!(geometry.rect_count, 2);
        let vertical = geometry.rects()[0];
        assert_f32_eq(vertical.left, 0.45);
        assert_f32_eq(vertical.top, 0.0);
        assert_f32_eq(vertical.right, 0.55);
        assert_f32_eq(vertical.bottom, 1.0);

        let horizontal = geometry.rects()[1];
        assert_f32_eq(horizontal.left, 0.0);
        assert_f32_eq(horizontal.top, 0.475);
        assert_f32_eq(horizontal.right, 1.0);
        assert_f32_eq(horizontal.bottom, 0.525);
    }

    #[test]
    fn box_draw_double_cross_geometry() {
        let geometry =
            box_draw_geometry_for_char('\u{256C}', 10.0, 20.0, 14.0).expect("expected geometry");

        assert_eq!(geometry.rect_count, 8);

        let top_left_vertical = geometry.rects()[0];
        assert_f32_eq(top_left_vertical.left, 0.35);
        assert_f32_eq(top_left_vertical.right, 0.45);
        assert_f32_eq(top_left_vertical.top, 0.0);
        assert_f32_eq(top_left_vertical.bottom, 0.475);

        let top_right_vertical = geometry.rects()[1];
        assert_f32_eq(top_right_vertical.left, 0.55);
        assert_f32_eq(top_right_vertical.right, 0.65);
        assert_f32_eq(top_right_vertical.top, 0.0);
        assert_f32_eq(top_right_vertical.bottom, 0.475);

        let top_right = geometry.rects()[2];
        assert_f32_eq(top_right.left, 0.55);
        assert_f32_eq(top_right.right, 1.0);
        assert_f32_eq(top_right.top, 0.425);
        assert_f32_eq(top_right.bottom, 0.475);

        let bottom_left = geometry.rects()[7];
        assert_f32_eq(bottom_left.left, 0.0);
        assert_f32_eq(bottom_left.right, 0.45);
        assert_f32_eq(bottom_left.top, 0.525);
        assert_f32_eq(bottom_left.bottom, 0.575);

        let bottom_right = geometry.rects()[3];
        assert_f32_eq(bottom_right.left, 0.55);
        assert_f32_eq(bottom_right.right, 1.0);
        assert_f32_eq(bottom_right.top, 0.525);
        assert_f32_eq(bottom_right.bottom, 0.575);
    }

    #[test]
    fn box_draw_light_to_heavy_connector_matches_ghostty_join_extents() {
        let geometry =
            box_draw_geometry_for_char('\u{251D}', 10.0, 20.0, 14.0).expect("expected geometry");

        assert_eq!(geometry.rect_count, 2);

        let vertical = geometry.rects()[0];
        assert_f32_eq(vertical.left, 0.45);
        assert_f32_eq(vertical.right, 0.55);
        assert_f32_eq(vertical.top, 0.0);
        assert_f32_eq(vertical.bottom, 1.0);

        let horizontal = geometry.rects()[1];
        assert_f32_eq(horizontal.left, 0.55);
        assert_f32_eq(horizontal.right, 1.0);
        assert_f32_eq(horizontal.top, 0.45);
        assert_f32_eq(horizontal.bottom, 0.55);
    }

    #[test]
    fn box_draw_light_to_double_connector_matches_ghostty_join_extents() {
        let geometry =
            box_draw_geometry_for_char('\u{255E}', 10.0, 20.0, 14.0).expect("expected geometry");

        assert_eq!(geometry.rect_count, 3);

        let vertical = geometry.rects()[0];
        assert_f32_eq(vertical.left, 0.45);
        assert_f32_eq(vertical.right, 0.55);
        assert_f32_eq(vertical.top, 0.0);
        assert_f32_eq(vertical.bottom, 1.0);

        let top_double = geometry.rects()[1];
        assert_f32_eq(top_double.left, 0.55);
        assert_f32_eq(top_double.right, 1.0);
        assert_f32_eq(top_double.top, 0.425);
        assert_f32_eq(top_double.bottom, 0.475);

        let bottom_double = geometry.rects()[2];
        assert_f32_eq(bottom_double.left, 0.55);
        assert_f32_eq(bottom_double.right, 1.0);
        assert_f32_eq(bottom_double.top, 0.525);
        assert_f32_eq(bottom_double.bottom, 0.575);
    }

    #[test]
    fn box_draw_lines_extend_to_cell_edges() {
        let vertical =
            box_draw_geometry_for_char('\u{2551}', 10.0, 20.0, 14.0).expect("expected geometry");
        assert!(
            vertical
                .rects()
                .iter()
                .all(|rect| rect.top == 0.0 && rect.bottom == 1.0)
        );

        let horizontal =
            box_draw_geometry_for_char('\u{2550}', 10.0, 20.0, 14.0).expect("expected geometry");
        assert!(
            horizontal
                .rects()
                .iter()
                .all(|rect| rect.left == 0.0 && rect.right == 1.0)
        );
    }

    #[test]
    fn rounded_top_left_corner_uses_ghostty_style_cubic_path() {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: Size {
                width: px(10.0),
                height: px(20.0),
            },
        };
        let spec =
            rounded_corner_path_spec(bounds, '\u{256D}', px(1.0)).expect("expected path points");

        assert_f32_eq(spec.start.x.into(), 5.5);
        assert_f32_eq(spec.start.y.into(), 20.0);
        assert_f32_eq(spec.curve_start.x.into(), 5.5);
        assert_f32_eq(spec.curve_start.y.into(), 15.0);
        assert_f32_eq(spec.control_a.x.into(), 5.5);
        assert_f32_eq(spec.control_a.y.into(), 11.625);
        assert_f32_eq(spec.control_b.x.into(), 6.625);
        assert_f32_eq(spec.control_b.y.into(), 10.5);
        assert_f32_eq(spec.curve_end.x.into(), 10.0);
        assert_f32_eq(spec.curve_end.y.into(), 10.5);
        assert_f32_eq(spec.end.x.into(), 10.0);
        assert_f32_eq(spec.end.y.into(), 10.5);
    }

    #[test]
    fn rounded_bottom_right_corner_uses_ghostty_style_cubic_path() {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: Size {
                width: px(20.0),
                height: px(10.0),
            },
        };
        let spec =
            rounded_corner_path_spec(bounds, '\u{256F}', px(1.0)).expect("expected path points");

        assert_f32_eq(spec.start.x.into(), 10.5);
        assert_f32_eq(spec.start.y.into(), 0.0);
        assert_f32_eq(spec.curve_start.x.into(), 10.5);
        assert_f32_eq(spec.curve_start.y.into(), 1.0);
        assert_f32_eq(spec.control_a.x.into(), 10.5);
        assert_f32_eq(spec.control_a.y.into(), 4.375);
        assert_f32_eq(spec.control_b.x.into(), 9.375);
        assert_f32_eq(spec.control_b.y.into(), 5.5);
        assert_f32_eq(spec.curve_end.x.into(), 6.0);
        assert_f32_eq(spec.curve_end.y.into(), 5.5);
        assert_f32_eq(spec.end.x.into(), 0.0);
        assert_f32_eq(spec.end.y.into(), 5.5);
    }

    #[test]
    fn diagonal_upper_right_to_lower_left_uses_ghostty_style_overshoot() {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: Size {
                width: px(10.0),
                height: px(20.0),
            },
        };
        let (spec, secondary) =
            diagonal_path_specs(bounds, '\u{2571}', px(1.0)).expect("expected path points");

        assert!(secondary.is_none());
        assert_f32_eq(spec.start.x.into(), 10.25);
        assert_f32_eq(spec.start.y.into(), -0.5);
        assert_f32_eq(spec.end.x.into(), -0.25);
        assert_f32_eq(spec.end.y.into(), 20.5);
    }

    #[test]
    fn diagonal_cross_emits_both_stroked_segments() {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: Size {
                width: px(10.0),
                height: px(20.0),
            },
        };
        let (primary, secondary) =
            diagonal_path_specs(bounds, '\u{2573}', px(1.0)).expect("expected path points");
        let secondary = secondary.expect("expected second diagonal");

        assert_f32_eq(primary.start.x.into(), 10.25);
        assert_f32_eq(primary.start.y.into(), -0.5);
        assert_f32_eq(primary.end.x.into(), -0.25);
        assert_f32_eq(primary.end.y.into(), 20.5);

        assert_f32_eq(secondary.start.x.into(), -0.25);
        assert_f32_eq(secondary.start.y.into(), -0.5);
        assert_f32_eq(secondary.end.x.into(), 10.25);
        assert_f32_eq(secondary.end.y.into(), 20.5);
    }

    #[test]
    fn batches_merge_adjacent_cells_with_same_style() {
        let grid = test_grid(vec![test_cell(0, 0, 'a'), test_cell(1, 0, 'b')], None);
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].row, 0);
        assert_eq!(batches[0].start_col, 0);
        assert_eq!(batches[0].text, "ab");
    }

    #[test]
    fn batches_split_on_row_change() {
        let grid = test_grid(vec![test_cell(0, 0, 'a'), test_cell(0, 1, 'b')], None);
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].text, "a");
        assert_eq!(batches[1].text, "b");
        assert_eq!(batches[0].row, 0);
        assert_eq!(batches[1].row, 1);
    }

    #[test]
    fn batches_split_on_bold_or_color_change() {
        let first = test_cell(0, 0, 'a');
        let mut second = test_cell(1, 0, 'b');
        let mut third = test_cell(2, 0, 'c');
        second.bold = true;
        third.fg = test_color(0.8, 0.4, 0.3);
        let grid = test_grid(vec![first, second, third], None);
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].text, "a");
        assert_eq!(batches[1].text, "b");
        assert_eq!(batches[2].text, "c");
    }

    #[test]
    fn batches_split_on_hover_underline_boundary() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, 'b'),
                test_cell(2, 0, 'c'),
            ],
            Some((0, 1, 2)),
        );
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].text, "a");
        assert!(batches[0].underline.is_none());
        assert_eq!(batches[1].text, "bc");
        assert!(batches[1].underline.is_some());
    }

    #[test]
    fn batches_keep_emoji_in_normal_text_flow() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, '📦'),
                test_cell(2, 0, 'b'),
            ],
            None,
        );
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].text, "a📦b");
        assert_eq!(batches[0].start_col, 0);
    }

    #[test]
    fn draw_ops_include_emoji_cells() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, '📦'),
                test_cell(2, 0, 'b'),
            ],
            None,
        );
        let ops = grid.collect_draw_ops(test_color(0.0, 0.0, 1.0), test_color(0.0, 0.0, 1.0));
        assert_eq!(ops.len(), 1);
        assert!(
            matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "a📦b" && batch.start_col == 0)
        );
    }

    #[test]
    fn batches_split_on_non_render_text_cells_and_controls() {
        let mut spacer = test_cell(1, 0, 'x');
        spacer.render_text = false;
        let mut control = test_cell(2, 0, '\u{001B}');
        control.render_text = true;
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                spacer,
                control,
                test_cell(3, 0, ' '),
                test_cell(4, 0, '\0'),
                test_cell(5, 0, 'b'),
            ],
            None,
        );
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].text, "a");
        assert_eq!(batches[1].text, "b");
    }

    #[test]
    fn batches_do_not_include_block_element_glyphs() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, '\u{2588}'),
                test_cell(2, 0, 'b'),
            ],
            None,
        );
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].text, "a");
        assert_eq!(batches[1].text, "b");
    }

    #[test]
    fn batches_break_around_wide_char_spacer_boundaries() {
        let mut spacer = test_cell(1, 0, ' ');
        spacer.render_text = false;
        let grid = test_grid(
            vec![test_cell(0, 0, '你'), spacer, test_cell(2, 0, 'x')],
            None,
        );
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].text, "你");
        assert_eq!(batches[1].text, "x");
    }

    #[test]
    fn draw_ops_interleave_text_and_block_in_cell_order() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, '\u{2588}'),
                test_cell(2, 0, 'b'),
            ],
            None,
        );
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "a"));
        assert!(matches!(&ops[1], TextDrawOp::Block(_)));
        assert!(matches!(&ops[2], TextDrawOp::Batch(batch) if batch.text == "b"));
    }

    #[test]
    fn draw_ops_flush_batch_before_block() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, 'b'),
                test_cell(2, 0, '\u{2588}'),
                test_cell(3, 0, 'c'),
                test_cell(4, 0, 'd'),
            ],
            None,
        );
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "ab"));
        assert!(matches!(&ops[1], TextDrawOp::Block(_)));
        assert!(matches!(&ops[2], TextDrawOp::Batch(batch) if batch.text == "cd"));
    }

    #[test]
    fn draw_ops_flush_batch_before_box_draw() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, 'b'),
                test_cell(2, 0, '\u{2502}'),
                test_cell(3, 0, 'c'),
                test_cell(4, 0, 'd'),
            ],
            None,
        );
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "ab"));
        assert!(matches!(&ops[1], TextDrawOp::Block(block) if block.col == 2));
        assert!(matches!(&ops[2], TextDrawOp::Batch(batch) if batch.text == "cd"));
    }

    #[test]
    fn draw_ops_emit_rounded_corner_variant() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, '\u{256D}'),
                test_cell(2, 0, 'b'),
            ],
            None,
        );
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "a"));
        assert!(
            matches!(&ops[1], TextDrawOp::RoundedCorner(corner) if corner.col == 1 && corner.glyph == '\u{256D}')
        );
        assert!(matches!(&ops[2], TextDrawOp::Batch(batch) if batch.text == "b"));
    }

    #[test]
    fn draw_ops_emit_diagonal_variant() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, '\u{2573}'),
                test_cell(2, 0, 'b'),
            ],
            None,
        );
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "a"));
        assert!(
            matches!(&ops[1], TextDrawOp::Diagonal(diagonal) if diagonal.col == 1 && diagonal.glyph == '\u{2573}')
        );
        assert!(matches!(&ops[2], TextDrawOp::Batch(batch) if batch.text == "b"));
    }

    #[test]
    fn draw_ops_skip_non_drawable_and_preserve_subsequent_order() {
        let mut spacer = test_cell(1, 0, 'x');
        spacer.render_text = false;
        let mut control = test_cell(3, 0, '\u{001B}');
        control.render_text = true;
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                spacer,
                test_cell(2, 0, '\u{2588}'),
                control,
                test_cell(4, 0, 'b'),
            ],
            None,
        );
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "a"));
        assert!(matches!(&ops[1], TextDrawOp::Block(_)));
        assert!(matches!(&ops[2], TextDrawOp::Batch(batch) if batch.text == "b"));
    }

    #[test]
    fn draw_ops_preserve_row_boundaries_with_blocks() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, 'b'),
                test_cell(0, 1, 'c'),
                test_cell(1, 1, '\u{2588}'),
                test_cell(2, 1, 'd'),
            ],
            None,
        );
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 4);
        assert!(
            matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "ab" && batch.row == 0)
        );
        assert!(matches!(&ops[1], TextDrawOp::Batch(batch) if batch.text == "c" && batch.row == 1));
        assert!(matches!(&ops[2], TextDrawOp::Block(block) if block.row == 1 && block.col == 1));
        assert!(matches!(&ops[3], TextDrawOp::Batch(batch) if batch.text == "d" && batch.row == 1));
    }

    #[test]
    fn block_draw_uses_same_fg_precedence_as_text() {
        let mut selected_text = test_cell(0, 0, 'x');
        selected_text.selected = true;
        let mut selected_block = test_cell(1, 0, '\u{2588}');
        selected_block.selected = true;
        let grid = test_grid(vec![selected_text, selected_block], None);
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 2);
        let text_fg = match &ops[0] {
            TextDrawOp::Batch(batch) => batch.fg,
            TextDrawOp::Block(_)
            | TextDrawOp::Sextant(_)
            | TextDrawOp::RoundedCorner(_)
            | TextDrawOp::Diagonal(_) => {
                panic!("expected text batch")
            }
        };
        let block_fg = match &ops[1] {
            TextDrawOp::Block(block) => block.fg,
            TextDrawOp::Batch(_)
            | TextDrawOp::Sextant(_)
            | TextDrawOp::RoundedCorner(_)
            | TextDrawOp::Diagonal(_) => {
                panic!("expected block draw")
            }
        };
        assert_eq!(text_fg, grid.selection_fg);
        assert_eq!(block_fg, grid.selection_fg);

        let mut cursor_block = test_cell(0, 0, '\u{2588}');
        cursor_block.selected = true;
        cursor_block.search_current = true;
        let mut grid = test_grid(vec![cursor_block], None);
        grid.cursor_cell = Some((0, 0));
        grid.cursor_visible = true;
        let ops = collect_draw_ops(&grid);
        let block_fg = match &ops[0] {
            TextDrawOp::Block(block) => block.fg,
            TextDrawOp::Batch(_)
            | TextDrawOp::Sextant(_)
            | TextDrawOp::RoundedCorner(_)
            | TextDrawOp::Diagonal(_) => {
                panic!("expected block draw")
            }
        };
        assert_eq!(
            block_fg,
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 1.0
            }
        );
    }

    #[test]
    fn dirty_rows_for_pass_includes_cursor_transition_rows() {
        let mut grid = test_grid(vec![test_cell(0, 0, 'a')], None);
        grid.rows = 5;
        grid.paint_damage = TerminalGridPaintDamage::Rows(vec![2usize].into());
        grid.cursor_cell = Some((0, 1));

        let mut cache = TerminalGridPaintCache {
            style_key: Some(grid.paint_style_key()),
            last_cursor_cell: Some((0, 4)),
            ..Default::default()
        };
        let (full, style_changed, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);
        assert!(!full);
        assert!(!style_changed);
        assert_eq!(&*dirty_rows, &[1usize, 2usize, 4usize]);
    }

    #[test]
    fn blink_only_does_not_dirty_rows_for_line_cursor() {
        // Line cursor: toggling cursor_visible should NOT mark the cursor row dirty,
        // since the cursor quad is painted as an overlay and row draw ops are unchanged.
        let mut grid = test_grid(vec![test_cell(0, 0, 'a')], None);
        grid.rows = 3;
        grid.paint_damage = TerminalGridPaintDamage::None;
        grid.cursor_cell = Some((0, 1));
        grid.cursor_visible = false; // blink off
        grid.cursor_style = TerminalCursorStyle::Line;

        let mut cache = TerminalGridPaintCache {
            style_key: Some(grid.paint_style_key()),
            last_cursor_cell: Some((0, 1)), // same position
            last_cursor_visible: true,      // was visible
            ..Default::default()
        };
        let (full, style_changed, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);
        assert!(!full);
        assert!(!style_changed);
        assert!(
            dirty_rows.is_empty(),
            "Line cursor blink should not dirty any rows"
        );
    }

    #[test]
    fn blink_only_dirties_cursor_row_for_block_cursor() {
        // Block cursor: toggling cursor_visible MUST mark the cursor row dirty,
        // since the text fg color at the cursor cell is baked into draw ops.
        let mut grid = test_grid(vec![test_cell(0, 0, 'a')], None);
        grid.rows = 3;
        grid.paint_damage = TerminalGridPaintDamage::None;
        grid.cursor_cell = Some((0, 1));
        grid.cursor_visible = false; // blink off
        grid.cursor_style = TerminalCursorStyle::Block;

        let mut cache = TerminalGridPaintCache {
            style_key: Some(grid.paint_style_key()),
            last_cursor_cell: Some((0, 1)), // same position
            last_cursor_visible: true,      // was visible
            ..Default::default()
        };
        let (full, style_changed, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);
        assert!(!full);
        assert!(!style_changed);
        assert_eq!(
            &*dirty_rows,
            &[1usize],
            "Block cursor blink must dirty the cursor row"
        );
    }

    #[test]
    fn dirty_rows_for_pass_includes_hover_transition_rows() {
        let mut grid = test_grid(vec![test_cell(0, 0, 'a')], Some((3, 1, 2)));
        grid.rows = 5;
        grid.paint_damage = TerminalGridPaintDamage::None;
        let mut cache = TerminalGridPaintCache {
            style_key: Some(grid.paint_style_key()),
            last_hovered_link_range: Some((1, 0, 0)),
            ..Default::default()
        };
        let (full, style_changed, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);
        assert!(!full);
        assert!(!style_changed);
        assert_eq!(&*dirty_rows, &[1usize, 3usize]);
    }

    #[test]
    fn dirty_rows_for_pass_forces_full_repaint_when_style_changes() {
        let grid = test_grid(vec![test_cell(0, 0, 'a')], None);
        let mut cache = TerminalGridPaintCache::default();
        let (full, style_changed, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);
        assert!(full);
        assert!(style_changed);
        assert!(dirty_rows.is_empty());
    }

    #[test]
    fn row_background_spans_merge_contiguous_cells_with_same_fill() {
        let mut first = test_cell(0, 0, 'a');
        let mut second = test_cell(1, 0, 'b');
        let mut third = test_cell(2, 0, 'c');
        let mut fourth = test_cell(3, 0, 'd');
        let mut fifth = test_cell(4, 0, 'e');
        let shared_bg = test_color(0.6, 0.3, 0.2);
        first.bg = shared_bg;
        second.bg = shared_bg;
        third.search_match = true;
        fourth.search_match = true;
        fifth.bg = Hsla::transparent_black();

        let grid = test_grid(vec![first, second, third, fourth, fifth], None);
        let mut spans = Vec::new();
        grid.build_row_background_spans_into(
            grid.cells[0].as_slice(),
            &mut HashMap::new(),
            &mut spans,
        );
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start_col, 0);
        assert_eq!(spans[0].end_col_exclusive, 2);
        assert_eq!(spans[0].color, shared_bg);
        assert_eq!(spans[1].start_col, 2);
        assert_eq!(spans[1].end_col_exclusive, 4);
        assert_eq!(spans[1].color, grid.search_match_bg);
    }

    #[test]
    fn row_background_spans_skip_default_background_that_matches_surface() {
        let mut default_bg_cell = test_cell(0, 0, 'a');
        let mut ansi_bg_cell = test_cell(1, 0, 'b');
        default_bg_cell.uses_terminal_default_bg = true;
        default_bg_cell.bg = test_color(0.2, 0.2, 0.2);
        ansi_bg_cell.bg = test_color(0.2, 0.2, 0.2);

        let mut grid = test_grid(vec![default_bg_cell, ansi_bg_cell], None);
        grid.terminal_surface_bg = test_color(0.2, 0.2, 0.2);
        let mut spans = Vec::new();
        grid.build_row_background_spans_into(
            grid.cells[0].as_slice(),
            &mut HashMap::new(),
            &mut spans,
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start_col, 1);
        assert_eq!(spans[0].end_col_exclusive, 2);
        assert_eq!(spans[0].color, test_color(0.2, 0.2, 0.2));
    }

    #[test]
    fn row_background_spans_include_transformed_default_background_cells() {
        let mut default_bg_cell = test_cell(0, 0, 'a');
        default_bg_cell.uses_terminal_default_bg = true;
        default_bg_cell.bg = test_color(0.2, 0.2, 0.2);

        let mut grid = test_grid(vec![default_bg_cell], None);
        grid.terminal_surface_bg = test_color(0.1, 0.1, 0.1);
        let mut spans = Vec::new();
        grid.build_row_background_spans_into(
            grid.cells[0].as_slice(),
            &mut HashMap::new(),
            &mut spans,
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start_col, 0);
        assert_eq!(spans[0].end_col_exclusive, 1);
        assert_eq!(spans[0].color, test_color(0.2, 0.2, 0.2));
    }

    #[test]
    fn upper_half_block_cells_keep_non_default_background_spans() {
        let mut half_block = test_cell(0, 0, '\u{2580}');
        half_block.bg = test_color(0.8, 0.4, 0.2);

        let grid = test_grid(vec![half_block], None);
        let mut spans = Vec::new();
        grid.build_row_background_spans_into(
            grid.cells[0].as_slice(),
            &mut HashMap::new(),
            &mut spans,
        );

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start_col, 0);
        assert_eq!(spans[0].end_col_exclusive, 1);
        assert_eq!(spans[0].color, test_color(0.8, 0.4, 0.2));
    }

    #[test]
    fn matching_previous_row_ops_ignores_row_index_for_shifted_content() {
        let old_grid = test_grid_rows(
            vec![vec![test_cell(0, 0, 'a')], vec![test_cell(0, 1, 'b')]],
            None,
        );
        let new_grid = test_grid_rows(
            vec![vec![test_cell(0, 0, 'b')], vec![test_cell(0, 1, 'c')]],
            None,
        );
        let cursor_fg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        };
        let highlight_fg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.08,
            a: 1.0,
        };

        let previous_row_ops = vec![
            old_grid.rebuild_cached_row_ops(
                old_grid.cells[0].as_slice(),
                cursor_fg,
                highlight_fg,
                &mut HashMap::new(),
            ),
            old_grid.rebuild_cached_row_ops(
                old_grid.cells[1].as_slice(),
                cursor_fg,
                highlight_fg,
                &mut HashMap::new(),
            ),
        ];
        let next_row_ops = new_grid.rebuild_cached_row_ops(
            new_grid.cells[0].as_slice(),
            cursor_fg,
            highlight_fg,
            &mut HashMap::new(),
        );

        assert_eq!(
            find_matching_previous_row_ops_index(0, &next_row_ops, &previous_row_ops),
            Some(1)
        );
    }

    #[test]
    fn matching_previous_row_ops_rejects_hover_style_mismatches() {
        let previous_grid = test_grid(vec![test_cell(0, 0, 'a')], Some((0, 0, 0)));
        let next_grid = test_grid(vec![test_cell(0, 0, 'a')], None);
        let cursor_fg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        };
        let highlight_fg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.08,
            a: 1.0,
        };

        let previous_row_ops = vec![previous_grid.rebuild_cached_row_ops(
            previous_grid.cells[0].as_slice(),
            cursor_fg,
            highlight_fg,
            &mut HashMap::new(),
        )];
        let next_row_ops = next_grid.rebuild_cached_row_ops(
            next_grid.cells[0].as_slice(),
            cursor_fg,
            highlight_fg,
            &mut HashMap::new(),
        );

        assert_eq!(
            find_matching_previous_row_ops_index(0, &next_row_ops, &previous_row_ops),
            None
        );
    }

    #[test]
    fn rebuild_cached_row_ops_initializes_pointer_sized_shaped_line_slots_per_draw_op() {
        let grid = test_grid(
            vec![
                test_cell(0, 0, 'a'),
                test_cell(1, 0, '\u{2588}'),
                test_cell(2, 0, 'b'),
            ],
            None,
        );
        let row_ops = grid.rebuild_cached_row_ops(
            grid.cells[0].as_slice(),
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 1.0,
            },
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.08,
                a: 1.0,
            },
            &mut HashMap::new(),
        );

        assert_eq!(row_ops.draw_ops.len(), 3);
        assert_eq!(row_ops.shaped_lines.len(), 3);
        assert!(row_ops.shaped_lines.iter().all(Option::is_none));
        assert_eq!(
            std::mem::size_of::<Option<Rc<ShapedLine>>>(),
            std::mem::size_of::<usize>(),
            "empty shaped-line cache slots must remain pointer-sized"
        );
    }

    #[test]
    fn rebuild_cached_rows_for_pass_clears_rows_missing_from_cells() {
        let mut grid = test_grid(vec![test_cell(0, 0, 'a')], None);
        grid.rows = 2;
        grid.cells = Arc::new(vec![Arc::new(vec![test_cell(0, 0, 'a')])]);

        let cursor_fg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        };
        let highlight_fg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.08,
            a: 1.0,
        };

        let stale_row_cells = vec![test_cell(0, 1, 'z')];
        let mut cache = TerminalGridPaintCache {
            row_ops: vec![
                CachedRowPaintOps::default(),
                grid.rebuild_cached_row_ops(
                    stale_row_cells.as_slice(),
                    cursor_fg,
                    highlight_fg,
                    &mut HashMap::new(),
                ),
            ],
            ..Default::default()
        };
        assert!(!cache.row_ops[1].draw_ops.is_empty());
        assert_eq!(
            cache.row_ops[1].shaped_lines.len(),
            cache.row_ops[1].draw_ops.len()
        );

        grid.rebuild_cached_rows_for_pass(
            &mut cache,
            false,
            false,
            &[1usize],
            cursor_fg,
            highlight_fg,
        );
        assert!(cache.row_ops[1].draw_ops.is_empty());
        assert!(cache.row_ops[1].background_spans.is_empty());
        assert!(cache.row_ops[1].shaped_lines.is_empty());
    }

    #[test]
    fn paint_cache_handle_clear_resets_seeded_rows() {
        let handle = TerminalGridPaintCacheHandle::default();
        handle.debug_seed_rows_for_tests(3);
        assert_eq!(handle.debug_row_cache_len_for_tests(), 3);
        handle.clear();
        assert_eq!(handle.debug_row_cache_len_for_tests(), 0);
    }

    #[test]
    fn dirty_rows_for_pass_row_ranges_extracts_rows_and_col_ranges() {
        let mut grid = test_grid(vec![test_cell(0, 0, 'a')], None);
        grid.rows = 5;
        grid.paint_damage = TerminalGridPaintDamage::RowRanges(vec![(1, 10, 20), (3, 5, 8)].into());

        let mut cache = TerminalGridPaintCache {
            style_key: Some(grid.paint_style_key()),
            ..Default::default()
        };
        cache.ensure_row_capacity(5);
        let (full, style_changed, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);

        assert!(!full);
        assert!(!style_changed);
        assert_eq!(&*dirty_rows, &[1usize, 3usize]);
        assert_eq!(cache.dirty_col_ranges[1], Some((10, 20)));
        assert_eq!(cache.dirty_col_ranges[3], Some((5, 8)));
        assert_eq!(cache.dirty_col_ranges[0], None);
        assert_eq!(cache.dirty_col_ranges[2], None);
    }

    #[test]
    fn dirty_rows_for_pass_row_ranges_merges_spans_on_same_row() {
        let mut grid = test_grid(vec![test_cell(0, 0, 'a')], None);
        grid.rows = 3;
        // Two spans on row 1: cols 5-10 and cols 15-20 → should merge to 5-20
        grid.paint_damage =
            TerminalGridPaintDamage::RowRanges(vec![(1, 5, 10), (1, 15, 20)].into());

        let mut cache = TerminalGridPaintCache {
            style_key: Some(grid.paint_style_key()),
            ..Default::default()
        };
        cache.ensure_row_capacity(3);
        let (_, _, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);

        // Row 1 appears once despite two spans
        assert_eq!(&*dirty_rows, &[1usize]);
        // Col ranges should be unioned: min(5,15)=5, max(10,20)=20
        assert_eq!(cache.dirty_col_ranges[1], Some((5, 20)));
    }

    #[test]
    fn draw_op_col_range_returns_correct_range_for_batch() {
        let batch = TextDrawOp::Batch(
            TextBatchBuilder::new(
                5, // start_col
                0, // row
                'a',
                TextBatchKey {
                    bold: false,
                    fg: Hsla::transparent_black(),
                },
                None,
            )
            .finalize(),
        );
        // Single char batch: range is (5, 5)
        assert_eq!(draw_op_col_range(&batch), (5, 5));
    }

    #[test]
    fn draw_op_col_range_returns_correct_range_for_block() {
        let block = TextDrawOp::Block(BlockDraw {
            row: 0,
            col: 7,
            geometry: block_element_geometry('\u{2580}').unwrap(),
            fg: Hsla::transparent_black(),
        });
        assert_eq!(draw_op_col_range(&block), (7, 7));
    }

    #[test]
    fn draw_op_col_range_returns_correct_range_for_sextant() {
        let sextant = TextDrawOp::Sextant(SextantDraw {
            row: 0,
            col: 4,
            packed: 0b01_1111,
            fg: Hsla::transparent_black(),
        });
        assert_eq!(draw_op_col_range(&sextant), (4, 4));
    }

    #[test]
    fn sextant_char_to_packed_matches_terminal_qr_decoding() {
        assert_eq!(sextant_char_to_packed('\u{1FB00}'), Some(0b01_1111));
        assert_eq!(sextant_char_to_packed('\u{1FB3B}'), Some(0b10_0000));
        assert_eq!(sextant_char_to_packed('█'), None);
        assert_eq!(sextant_char_to_packed('\u{1FAFF}'), None);
        assert_eq!(sextant_char_to_packed('\u{1FB3C}'), None);
    }

    #[test]
    fn draw_ops_emit_sextant_for_legacy_mosaic() {
        let grid = test_grid(
            vec![test_cell(0, 0, 'a'), test_cell(1, 0, '\u{1FB00}')],
            None,
        );
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "a"));
        assert!(
            matches!(&ops[1], TextDrawOp::Sextant(s) if s.row == 0 && s.col == 1 && s.packed == 0b01_1111)
        );
    }

    #[test]
    fn col_ranges_overlap_detects_overlapping_ranges() {
        assert!(col_ranges_overlap((0, 5), (3, 8)));
        assert!(col_ranges_overlap((3, 8), (0, 5)));
        assert!(col_ranges_overlap((5, 5), (5, 5)));
        assert!(col_ranges_overlap((0, 10), (5, 5)));
    }

    #[test]
    fn col_ranges_overlap_detects_non_overlapping_ranges() {
        assert!(!col_ranges_overlap((0, 4), (5, 10)));
        assert!(!col_ranges_overlap((5, 10), (0, 4)));
        assert!(!col_ranges_overlap((0, 0), (1, 1)));
    }

    #[test]
    fn dirty_rows_for_pass_row_ranges_resets_each_pass() {
        // Verify that dirty_col_ranges is cleared between passes (via ensure_row_capacity)
        let mut grid = test_grid(vec![test_cell(0, 0, 'a')], None);
        grid.rows = 3;
        grid.paint_damage = TerminalGridPaintDamage::RowRanges(vec![(1, 5, 10)].into());

        let mut cache = TerminalGridPaintCache {
            style_key: Some(grid.paint_style_key()),
            ..Default::default()
        };
        cache.ensure_row_capacity(3);
        grid.dirty_rows_for_pass(&mut cache);
        assert_eq!(cache.dirty_col_ranges[1], Some((5, 10)));

        // Second pass with different damage — must not carry over previous col range
        grid.paint_damage = TerminalGridPaintDamage::None;
        cache.ensure_row_capacity(3);
        let (_, _, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);
        assert!(dirty_rows.is_empty());
        assert_eq!(
            cache.dirty_col_ranges[1], None,
            "col ranges must reset each pass"
        );
    }
}
