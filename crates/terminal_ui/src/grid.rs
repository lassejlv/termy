use gpui::{
    App, Bounds, Element, Font, FontFeatures, FontStyle, FontWeight, Hsla, IntoElement,
    PathBuilder, Pixels, ShapedLine, SharedString, Size, StrikethroughStyle, TextRun,
    UnderlineStyle as GpuiUnderlineStyle, Window, point, px, quad,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc, time::Instant};
use termy_core::{
    TerminalCursorStyle, TerminalGlyphMetrics, TerminalGlyphNeighbors, TerminalGlyphPlan,
    TerminalGlyphRect, TerminalGlyphRectSnap, TerminalGlyphRenderKind, TerminalGlyphStrokeKind,
    add_span_grid_paint_us, add_span_row_ops_rebuild_us, add_span_text_shaping_us,
    increment_grid_paint_count, increment_shape_line_calls, increment_shaped_line_cache_hit,
    increment_shaped_line_cache_miss, terminal_glyph_plan, terminal_ui_render_metrics_enabled,
};

/// The visual form of a terminal underline requested by SGR 4 variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalUnderlineStyle {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// Semantic underline decoration for a terminal cell.
///
/// A missing color means that the cell's resolved foreground color is used.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalUnderline {
    pub style: TerminalUnderlineStyle,
    pub color: Option<Hsla>,
}

/// Info needed to render a single cell.
#[derive(Clone)]
pub struct CellRenderInfo {
    pub col: usize,
    pub char: char,
    pub combining: Option<SharedString>,
    pub fg: Hsla,
    pub bg: Hsla,
    pub uses_terminal_default_bg: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: Option<TerminalUnderline>,
    pub strikethrough: bool,
    pub render_text: bool,
    /// Trailing cell occupied by a two-column character.
    pub wide_character_spacer: bool,
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
    /// Hovered link range as `(start_row, start_col, end_row, end_col)`.
    pub hovered_link_range: Option<(usize, usize, usize, usize)>,
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
// NOTE: We also render most Unicode box-drawing characters (U+2500..U+257F) as
// pixel-snapped geometry instead of shaped font glyphs.
//
// Why:
// - Font glyphs are sized to the font's natural cell height, not the terminal's
//   cell height. When line_height > 1.0, this leaves visible gaps between rows.
// - Even at line_height = 1.0, built-in rendering gives crisper and more
//   consistent results across fonts, for the same reasons as block elements.
// - This mirrors Ghostty's sprite-rendering approach for straight box lines.
//
// Exceptions that do not use this geometry:
// - Rounded corners (U+256D-U+2570) use explicit cubic paths sized to the full
//   terminal cell so they meet adjacent straight lines without gaps.
// - Diagonals (U+2571-U+2573) remain explicit stroked paths so adjacent cells
//   join without gaps.
#[cfg(test)]
const BOX_DRAWING_START: u32 = 0x2500;
#[cfg(test)]
const BOX_DRAWING_END: u32 = 0x257F;
#[cfg(test)]
const BLOCK_ELEMENTS_START: u32 = 0x2580;
#[cfg(test)]
const BLOCK_ELEMENTS_END: u32 = 0x259F;

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

#[cfg(test)]
fn test_glyph_metrics(cell_width: f32, cell_height: f32, font_size: f32) -> TerminalGlyphMetrics {
    TerminalGlyphMetrics {
        cell_width,
        cell_height,
        font_size,
    }
}

#[cfg(test)]
fn block_element_geometry(character: char) -> Option<TerminalGlyphPlan> {
    let plan = terminal_glyph_plan(
        character,
        test_glyph_metrics(10.0, 20.0, 14.0),
        TerminalGlyphNeighbors::default(),
    )?;
    (plan.kind() == TerminalGlyphRenderKind::BlockElement).then_some(plan)
}

#[cfg(test)]
fn braille_geometry(character: char) -> Option<TerminalGlyphPlan> {
    let plan = terminal_glyph_plan(
        character,
        test_glyph_metrics(10.0, 20.0, 14.0),
        TerminalGlyphNeighbors {
            before: Some(character),
            after: Some(character),
            ..TerminalGlyphNeighbors::default()
        },
    )?;
    (plan.kind() == TerminalGlyphRenderKind::Braille).then_some(plan)
}

#[cfg(test)]
fn box_draw_geometry_for_char(
    character: char,
    cell_width: f32,
    cell_height: f32,
    font_size: f32,
) -> Option<TerminalGlyphPlan> {
    let plan = terminal_glyph_plan(
        character,
        test_glyph_metrics(cell_width, cell_height, font_size),
        TerminalGlyphNeighbors::default(),
    )?;
    (plan.kind() == TerminalGlyphRenderKind::BoxDrawing).then_some(plan)
}

#[cfg(test)]
fn sextant_geometry(character: char) -> Option<TerminalGlyphPlan> {
    let plan = terminal_glyph_plan(
        character,
        test_glyph_metrics(10.0, 20.0, 14.0),
        TerminalGlyphNeighbors::default(),
    )?;
    (plan.kind() == TerminalGlyphRenderKind::Sextant).then_some(plan)
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
    italic: bool,
    strikethrough: bool,
    fg: Hsla,
    underline: Option<TerminalUnderline>,
    cell_len: usize,
}

#[derive(Clone, Copy)]
struct BlockDraw {
    #[cfg_attr(not(test), allow(dead_code))]
    row: usize,
    col: usize,
    geometry: TerminalGlyphPlan,
    fg: Hsla,
}

#[derive(Clone, Copy)]
struct SextantDraw {
    #[cfg_attr(not(test), allow(dead_code))]
    row: usize,
    col: usize,
    geometry: TerminalGlyphPlan,
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
    last_hovered_link_range: Option<(usize, usize, usize, usize)>,
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
    cached_font_italic: Option<Font>,
    cached_font_bold_italic: Option<Font>,
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
    italic: bool,
    strikethrough: bool,
    fg: Hsla,
}

/// Temporary mutable builder for a text batch. Collects chars into a String,
/// then converts to the immutable `TextBatch` (with `SharedString`) on finalize.
struct TextBatchBuilder {
    start_col: usize,
    row: usize,
    text: String,
    bold: bool,
    italic: bool,
    strikethrough: bool,
    fg: Hsla,
    underline: Option<TerminalUnderline>,
    cell_len: usize,
}

impl TextBatchBuilder {
    fn new(
        start_col: usize,
        row: usize,
        initial_char: char,
        initial_combining: Option<&str>,
        key: TextBatchKey,
        underline: Option<TerminalUnderline>,
    ) -> Self {
        let mut text = String::with_capacity(16);
        text.push(initial_char);
        if let Some(combining) = initial_combining {
            text.push_str(combining);
        }
        Self {
            start_col,
            row,
            text,
            bold: key.bold,
            italic: key.italic,
            strikethrough: key.strikethrough,
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
        underline: &Option<TerminalUnderline>,
    ) -> bool {
        self.row == row
            && self.start_col + self.cell_len == col
            && self.bold == key.bold
            && self.italic == key.italic
            && self.strikethrough == key.strikethrough
            && self.fg == key.fg
            && self.underline == *underline
    }

    fn append_cell(&mut self, c: char, combining: Option<&str>) {
        self.text.push(c);
        if let Some(combining) = combining {
            self.text.push_str(combining);
        }
        self.cell_len += 1;
    }

    fn finalize(self) -> TextBatch {
        TextBatch {
            start_col: self.start_col,
            row: self.row,
            text: SharedString::from(self.text),
            bold: self.bold,
            italic: self.italic,
            strikethrough: self.strikethrough,
            fg: self.fg,
            underline: self.underline,
            cell_len: self.cell_len,
        }
    }
}

fn snapped_block_rect_bounds(
    cell_bounds: Bounds<Pixels>,
    rect: TerminalGlyphRect,
) -> Option<Bounds<Pixels>> {
    let origin_x: f32 = cell_bounds.origin.x.into();
    let origin_y: f32 = cell_bounds.origin.y.into();
    let cell_width: f32 = cell_bounds.size.width.into();
    let cell_height: f32 = cell_bounds.size.height.into();

    let transformed_left = origin_x + cell_width * rect.left;
    let transformed_right = origin_x + cell_width * rect.right;
    let transformed_top = origin_y + cell_height * rect.top;
    let transformed_bottom = origin_y + cell_height * rect.bottom;
    let (left, right, top, bottom) = match rect.snap {
        TerminalGlyphRectSnap::Nearest => (
            transformed_left.round(),
            transformed_right.round(),
            transformed_top.round(),
            transformed_bottom.round(),
        ),
        TerminalGlyphRectSnap::Outward => (
            transformed_left.floor(),
            transformed_right.ceil(),
            transformed_top.floor(),
            transformed_bottom.ceil(),
        ),
    };

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
    geometry: &TerminalGlyphPlan,
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

fn paint_rounded_corner_path(
    window: &mut Window,
    cell_bounds: Bounds<Pixels>,
    glyph: char,
    color: Hsla,
    font_size: Pixels,
) {
    paint_terminal_glyph_strokes(window, cell_bounds, glyph, color, font_size);
}

fn paint_diagonal_path(
    window: &mut Window,
    cell_bounds: Bounds<Pixels>,
    glyph: char,
    color: Hsla,
    font_size: Pixels,
) {
    paint_terminal_glyph_strokes(window, cell_bounds, glyph, color, font_size);
}

fn paint_terminal_glyph_strokes(
    window: &mut Window,
    cell_bounds: Bounds<Pixels>,
    glyph: char,
    color: Hsla,
    font_size: Pixels,
) {
    let Some(cell_bounds) = snapped_quad_bounds(cell_bounds) else {
        return;
    };
    let cell_width: f32 = cell_bounds.size.width.into();
    let cell_height: f32 = cell_bounds.size.height.into();
    let metrics = TerminalGlyphMetrics {
        cell_width,
        cell_height,
        font_size: font_size.into(),
    };
    let Some(plan) = terminal_glyph_plan(glyph, metrics, TerminalGlyphNeighbors::default()) else {
        return;
    };
    let resolve_point = |value: termy_core::TerminalGlyphPoint| {
        point(
            cell_bounds.origin.x + cell_bounds.size.width * value.x,
            cell_bounds.origin.y + cell_bounds.size.height * value.y,
        )
    };

    for stroke in plan.strokes() {
        let points = stroke.points();
        let mut builder = PathBuilder::stroke(px(cell_width * stroke.width));
        match stroke.kind {
            TerminalGlyphStrokeKind::Line if points.len() == 2 => {
                builder.move_to(resolve_point(points[0]));
                builder.line_to(resolve_point(points[1]));
            }
            TerminalGlyphStrokeKind::RoundedCorner if points.len() == 6 => {
                builder.move_to(resolve_point(points[0]));
                builder.line_to(resolve_point(points[1]));
                builder.cubic_bezier_to(
                    resolve_point(points[4]),
                    resolve_point(points[2]),
                    resolve_point(points[3]),
                );
                builder.line_to(resolve_point(points[5]));
            }
            TerminalGlyphStrokeKind::Line | TerminalGlyphStrokeKind::RoundedCorner => continue,
        }

        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CustomUnderlinePattern {
    Solid,
    Dotted,
    Dashed,
}

/// Pixel-snapped geometry for underline styles that GPUI does not provide.
///
/// Every decoration is emitted as a single path primitive. `line_count` is at
/// most two, so painting cost stays bounded per text batch even for wide rows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CustomUnderlinePathSpec {
    start_x: f32,
    end_x: f32,
    line_y: [f32; 2],
    line_count: usize,
    pattern: CustomUnderlinePattern,
}

fn custom_underline_path_spec(
    bounds: Bounds<Pixels>,
    underline_origin_y: Pixels,
    style: TerminalUnderlineStyle,
) -> Option<CustomUnderlinePathSpec> {
    let origin_x: f32 = bounds.origin.x.into();
    let origin_y: f32 = bounds.origin.y.into();
    let width: f32 = bounds.size.width.into();
    let height: f32 = bounds.size.height.into();
    let left = origin_x.round();
    let right = (origin_x + width).round();
    let top = origin_y.round();
    let bottom = (origin_y + height).round();

    if right - left < 1.0 || bottom - top < 1.0 {
        return None;
    }

    // Half-pixel centers keep a one-pixel stroke inside the snapped batch
    // bounds. Use the same baseline-relative origin as GPUI's text painter.
    let start_x = left + 0.5;
    let end_x = (right - 0.5).max(start_x);
    let min_y = top + 0.5;
    let underline_origin_y: f32 = underline_origin_y.into();
    let lower_y = (underline_origin_y.round() + 0.5).clamp(min_y, bottom - 0.5);

    match style {
        TerminalUnderlineStyle::Double => {
            let upper_y = (bottom - 3.5).max(min_y).min(lower_y);
            let line_count = if upper_y < lower_y { 2 } else { 1 };
            Some(CustomUnderlinePathSpec {
                start_x,
                end_x,
                line_y: [upper_y, lower_y],
                line_count,
                pattern: CustomUnderlinePattern::Solid,
            })
        }
        TerminalUnderlineStyle::Dotted => Some(CustomUnderlinePathSpec {
            start_x,
            end_x,
            line_y: [lower_y, lower_y],
            line_count: 1,
            pattern: CustomUnderlinePattern::Dotted,
        }),
        TerminalUnderlineStyle::Dashed => Some(CustomUnderlinePathSpec {
            start_x,
            end_x,
            line_y: [lower_y, lower_y],
            line_count: 1,
            pattern: CustomUnderlinePattern::Dashed,
        }),
        TerminalUnderlineStyle::Single | TerminalUnderlineStyle::Curly => None,
    }
}

fn paint_custom_underline(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    underline_origin_y: Pixels,
    underline: TerminalUnderline,
    fallback_color: Hsla,
) {
    let Some(spec) = custom_underline_path_spec(bounds, underline_origin_y, underline.style) else {
        return;
    };

    let mut builder = PathBuilder::stroke(px(1.0));
    builder = match spec.pattern {
        CustomUnderlinePattern::Solid => builder,
        CustomUnderlinePattern::Dotted => builder.dash_array(&[px(1.0), px(1.0)]),
        CustomUnderlinePattern::Dashed => builder.dash_array(&[px(3.0), px(2.0)]),
    };

    for y in spec.line_y.into_iter().take(spec.line_count) {
        builder.move_to(point(px(spec.start_x), px(y)));
        builder.line_to(point(px(spec.end_x), px(y)));
    }

    if let Ok(path) = builder.build() {
        window.paint_path(path, underline.color.unwrap_or(fallback_color));
    }
}

fn gpui_underline_style(
    underline: TerminalUnderline,
    fallback_color: Hsla,
) -> Option<GpuiUnderlineStyle> {
    let wavy = match underline.style {
        TerminalUnderlineStyle::Single => false,
        TerminalUnderlineStyle::Curly => true,
        TerminalUnderlineStyle::Double
        | TerminalUnderlineStyle::Dotted
        | TerminalUnderlineStyle::Dashed => return None,
    };

    Some(GpuiUnderlineStyle {
        thickness: px(1.0),
        color: Some(underline.color.unwrap_or(fallback_color)),
        wavy,
    })
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
        && lhs.italic == rhs.italic
        && lhs.strikethrough == rhs.strikethrough
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
    lhs.col == rhs.col && lhs.geometry == rhs.geometry && lhs.fg == rhs.fg
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
        row: usize,
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
        let metrics = TerminalGlyphMetrics {
            cell_width: cell_w,
            cell_height: cell_h,
            font_size: font_sz,
        };

        for (index, cell) in row_cells.iter().enumerate() {
            if !Self::cell_is_drawable_text(cell) {
                Self::push_pending_text_batch(&mut current, ops);
                continue;
            }

            let fg = self.cell_fg_color(row, cell, cursor_fg, highlight_fg);
            let char_at = |candidate: Option<usize>| {
                candidate
                    .and_then(|candidate| row_cells.get(candidate))
                    .map(|cell| cell.char)
            };
            let neighbors = TerminalGlyphNeighbors {
                two_before: char_at(index.checked_sub(2)),
                before: char_at(index.checked_sub(1)),
                after: char_at(index.checked_add(1)),
                two_after: char_at(index.checked_add(2)),
            };
            if cell.combining.is_none()
                && let Some(geometry) = terminal_glyph_plan(cell.char, metrics, neighbors)
            {
                Self::push_pending_text_batch(&mut current, ops);
                let operation = match geometry.kind() {
                    TerminalGlyphRenderKind::RoundedCorner => {
                        TextDrawOp::RoundedCorner(RoundedCornerDraw {
                            row,
                            col: cell.col,
                            glyph: cell.char,
                            fg,
                        })
                    }
                    TerminalGlyphRenderKind::Diagonal => TextDrawOp::Diagonal(DiagonalDraw {
                        row,
                        col: cell.col,
                        glyph: cell.char,
                        fg,
                    }),
                    TerminalGlyphRenderKind::Sextant => TextDrawOp::Sextant(SextantDraw {
                        row,
                        col: cell.col,
                        geometry,
                        fg,
                    }),
                    TerminalGlyphRenderKind::BlockElement
                    | TerminalGlyphRenderKind::BoxDrawing
                    | TerminalGlyphRenderKind::Braille => TextDrawOp::Block(BlockDraw {
                        row,
                        col: cell.col,
                        geometry,
                        fg,
                    }),
                };
                ops.push(operation);
                continue;
            }

            let underline = self.cell_underline(row, cell.col, fg, cell.underline);
            let key = TextBatchKey {
                bold: cell.bold,
                italic: cell.italic,
                strikethrough: cell.strikethrough,
                fg,
            };

            let should_append = current
                .as_ref()
                .is_some_and(|batch| batch.can_append(cell.col, row, key, &underline));
            if should_append {
                if let Some(batch) = current.as_mut() {
                    batch.append_cell(cell.char, cell.combining.as_deref().map(|text| &**text));
                }
                continue;
            }

            Self::push_pending_text_batch(&mut current, ops);
            current = Some(TextBatchBuilder::new(
                cell.col,
                row,
                cell.char,
                cell.combining.as_deref().map(|text| &**text),
                key,
                underline,
            ));
        }

        Self::push_pending_text_batch(&mut current, ops);
    }

    #[cfg(test)]
    fn rebuild_cached_row_ops_into(
        &self,
        row: usize,
        row_cells: &[CellRenderInfo],
        cursor_fg: Hsla,
        highlight_fg: Hsla,
        color_cache: &mut HashMap<[u32; 4], Option<Hsla>>,
        scratch_bg: &mut Vec<BackgroundSpan>,
        scratch_ops: &mut Vec<TextDrawOp>,
    ) -> CachedRowPaintOps {
        self.collect_row_draw_ops_into(row, row_cells, cursor_fg, highlight_fg, scratch_ops);
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
        row: usize,
        row_cells: &[CellRenderInfo],
        cursor_fg: Hsla,
        highlight_fg: Hsla,
        color_cache: &mut HashMap<[u32; 4], Option<Hsla>>,
    ) -> CachedRowPaintOps {
        let mut scratch_bg = Vec::new();
        let mut scratch_ops = Vec::new();
        self.rebuild_cached_row_ops_into(
            row,
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
        font_italic: &Font,
        font_bold_italic: &Font,
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
                        let font = match (batch.bold, batch.italic) {
                            (false, false) => font_normal,
                            (true, false) => font_bold,
                            (false, true) => font_italic,
                            (true, true) => font_bold_italic,
                        };
                        let run = TextRun {
                            len: batch.text.len(),
                            font: font.clone(),
                            color: batch.fg,
                            background_color: None,
                            underline: batch
                                .underline
                                .and_then(|underline| gpui_underline_style(underline, batch.fg)),
                            strikethrough: batch.strikethrough.then_some(StrikethroughStyle {
                                thickness: px(1.0),
                                color: Some(batch.fg),
                            }),
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
                    if let Some(underline) = batch.underline
                        && matches!(
                            underline.style,
                            TerminalUnderlineStyle::Double
                                | TerminalUnderlineStyle::Dotted
                                | TerminalUnderlineStyle::Dashed
                        )
                    {
                        let padding_top = (self.cell_size.height - line.ascent - line.descent) / 2.;
                        let underline_origin_y =
                            origin.y + padding_top + line.ascent + line.descent * 0.618;
                        paint_custom_underline(
                            window,
                            Bounds {
                                origin: point(x, origin.y),
                                size: Size {
                                    width: self.cell_size.width * batch.cell_len as f32,
                                    height: self.cell_size.height,
                                },
                            },
                            underline_origin_y,
                            underline,
                            batch.fg,
                        );
                    }
                    // Keep custom decorations below glyphs, matching GPUI's
                    // built-in underline paint order.
                    let _ = line.paint(point(x, origin.y), self.cell_size.height, window, cx);
                }
                TextDrawOp::Block(block) => {
                    let x = origin.x + self.cell_size.width * block.col as f32;
                    let cell_bounds = Bounds {
                        origin: point(x, origin.y),
                        size: self.cell_size,
                    };
                    paint_block_element_quad(window, cell_bounds, &block.geometry, block.fg);
                }
                TextDrawOp::Sextant(sextant) => {
                    let x = origin.x + self.cell_size.width * sextant.col as f32;
                    let cell_bounds = Bounds {
                        origin: point(x, origin.y),
                        size: self.cell_size,
                    };
                    paint_block_element_quad(window, cell_bounds, &sextant.geometry, sextant.fg);
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
            for range in [cache.last_hovered_link_range, self.hovered_link_range]
                .into_iter()
                .flatten()
            {
                let (start_row, _, end_row, _) = range;
                rows.extend(start_row..=end_row.min(self.rows.saturating_sub(1)));
            }
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

    fn cursor_bounds_for_row(
        &self,
        row: usize,
        origin: gpui::Point<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        let (cursor_col, cursor_row) = self.cursor_cell?;
        if !self.cursor_visible {
            return None;
        }
        if cursor_row != row {
            return None;
        }
        let x = origin.x + self.cell_size.width * cursor_col as f32;
        let y = origin.y;
        let cell_bounds = Bounds {
            origin: point(x, y),
            size: self.cell_size,
        };
        let cursor_bounds = match self.cursor_style {
            TerminalCursorStyle::Block => {
                // Use the terminal's trailing-cell marker, including emoji and
                // combined text, rather than guessing width from Unicode alone.
                let wide = self.cells.get(row).is_some_and(|cells| {
                    cells
                        .binary_search_by_key(&(cursor_col + 1), |cell| cell.col)
                        .ok()
                        .is_some_and(|index| cells[index].wide_character_spacer)
                });
                Bounds::new(
                    cell_bounds.origin,
                    Size {
                        width: self.cell_size.width * if wide { 2.0 } else { 1.0 },
                        height: self.cell_size.height,
                    },
                )
            }
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

        Some(cursor_bounds)
    }

    fn paint_cursor_for_row(&self, row: usize, origin: gpui::Point<Pixels>, window: &mut Window) {
        let Some(bounds) = self.cursor_bounds_for_row(row, origin) else {
            return;
        };
        window.paint_quad(quad(
            bounds,
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
                    row,
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
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            });
            cache.cached_font_bold = Some(Font {
                family: self.font_family.clone(),
                features: terminal_font_features.clone(),
                fallbacks: None,
                weight: FontWeight::BOLD,
                style: FontStyle::Normal,
            });
            cache.cached_font_italic = Some(Font {
                family: self.font_family.clone(),
                features: terminal_font_features.clone(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Italic,
            });
            cache.cached_font_bold_italic = Some(Font {
                family: self.font_family.clone(),
                features: terminal_font_features,
                fallbacks: None,
                weight: FontWeight::BOLD,
                style: FontStyle::Italic,
            });
        }
        let font_normal = cache.cached_font_normal.clone().unwrap();
        let font_bold = cache.cached_font_bold.clone().unwrap();
        let font_italic = cache.cached_font_italic.clone().unwrap();
        let font_bold_italic = cache.cached_font_bold_italic.clone().unwrap();

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
                &font_italic,
                &font_bold_italic,
            );
        }

        drop(cache);
    }

    #[cfg(test)]
    fn cell_count(&self) -> usize {
        self.cells.iter().map(|row| row.len()).sum()
    }

    fn cell_is_drawable_text(cell: &CellRenderInfo) -> bool {
        cell.render_text
            && (cell.char != ' '
                || cell.combining.is_some()
                || cell.underline.is_some()
                || cell.strikethrough)
            && cell.char != '\0'
            && !cell.char.is_control()
    }

    fn cell_fg_color(
        &self,
        row: usize,
        cell: &CellRenderInfo,
        cursor_fg: Hsla,
        highlight_fg: Hsla,
    ) -> Hsla {
        if self.cursor_cell == Some((cell.col, row))
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

    fn cell_underline(
        &self,
        row: usize,
        col: usize,
        color: Hsla,
        terminal_underline: Option<TerminalUnderline>,
    ) -> Option<TerminalUnderline> {
        let hovered =
            self.hovered_link_range
                .is_some_and(|(start_row, start_col, end_row, end_col)| {
                    if start_row == end_row {
                        row == start_row && col >= start_col && col <= end_col
                    } else if row == start_row {
                        col >= start_col
                    } else if row == end_row {
                        col <= end_col
                    } else {
                        row > start_row && row < end_row
                    }
                });
        terminal_underline.or_else(|| {
            hovered.then_some(TerminalUnderline {
                style: TerminalUnderlineStyle::Single,
                color: Some(color),
            })
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
        for (row, row_cells) in self.cells.iter().enumerate() {
            self.collect_row_draw_ops_into(
                row,
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
    fn block_cursor_covers_entire_wide_character() {
        let mut spacer = test_cell(2, ' ');
        spacer.render_text = false;
        spacer.wide_character_spacer = true;
        let mut grid = test_grid(vec![test_cell(0, 'a'), test_cell(1, 'に'), spacer], None);
        grid.cursor_cell = Some((1, 0));
        grid.cursor_visible = true;
        let bounds = grid
            .cursor_bounds_for_row(0, point(px(5.0), px(7.0)))
            .unwrap();
        assert_eq!(bounds.origin, point(px(15.0), px(7.0)));
        assert_eq!(
            bounds.size,
            Size {
                width: px(20.0),
                height: px(20.0)
            }
        );
        let cursor_fg = test_color(0.0, 0.0, 0.0);
        let batches = grid.collect_draw_ops(cursor_fg, cursor_fg);
        assert!(batches.iter().any(|op| matches!(op,
            TextDrawOp::Batch(batch) if batch.text == "に" && batch.fg == cursor_fg)));
        grid.cursor_style = TerminalCursorStyle::Line;
        assert_eq!(
            grid.cursor_bounds_for_row(0, bounds.origin)
                .unwrap()
                .size
                .width,
            px(2.0)
        );
    }

    #[test]
    fn cursor_bounds_preserve_narrow_line_and_hidden_cursors() {
        let mut grid = test_grid(vec![test_cell(0, 'a'), test_cell(1, ' ')], None);
        grid.cursor_cell = Some((0, 0));
        grid.cursor_visible = true;
        let origin = point(px(0.0), px(0.0));
        assert_eq!(
            grid.cursor_bounds_for_row(0, origin).unwrap().size.width,
            px(10.0)
        );
        grid.cursor_cell = Some((1, 0));
        assert_eq!(
            grid.cursor_bounds_for_row(0, origin).unwrap().size.width,
            px(10.0)
        );
        grid.cursor_style = TerminalCursorStyle::Line;
        assert_eq!(
            grid.cursor_bounds_for_row(0, origin).unwrap().size.width,
            px(2.0)
        );
        assert!(grid.cursor_bounds_for_row(1, origin).is_none());
        grid.cursor_visible = false;
        assert!(grid.cursor_bounds_for_row(0, origin).is_none());
        grid.cursor_cell = None;
        assert!(grid.cursor_bounds_for_row(0, origin).is_none());
    }

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

    fn test_cell(col: usize, c: char) -> CellRenderInfo {
        CellRenderInfo {
            col,
            char: c,
            combining: None,
            fg: test_color(0.4, 0.5, 0.6),
            bg: test_color(0.0, 0.0, 0.0),
            uses_terminal_default_bg: false,
            bold: false,
            italic: false,
            underline: None,
            strikethrough: false,
            render_text: true,
            wide_character_spacer: false,
            selected: false,
            search_current: false,
            search_match: false,
        }
    }

    fn test_grid(
        cells: Vec<CellRenderInfo>,
        hovered: Option<(usize, usize, usize, usize)>,
    ) -> TerminalGrid {
        test_grid_rows(vec![cells], hovered)
    }

    fn test_grid_rows(
        rows: Vec<Vec<CellRenderInfo>>,
        hovered: Option<(usize, usize, usize, usize)>,
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
                TextDrawOp::Block(_) | TextDrawOp::Sextant(_) | TextDrawOp::RoundedCorner(_) => {
                    None
                }
                TextDrawOp::Diagonal(_) => None,
            })
            .collect()
    }

    fn glyph_plan_for_bounds(
        bounds: Bounds<Pixels>,
        glyph: char,
    ) -> (Bounds<Pixels>, TerminalGlyphPlan) {
        let bounds = snapped_quad_bounds(bounds).expect("snapped cell bounds");
        let plan = terminal_glyph_plan(
            glyph,
            TerminalGlyphMetrics {
                cell_width: bounds.size.width.into(),
                cell_height: bounds.size.height.into(),
                font_size: 14.0,
            },
            TerminalGlyphNeighbors::default(),
        )
        .expect("special glyph plan");
        (bounds, plan)
    }

    fn resolve_glyph_point(
        bounds: Bounds<Pixels>,
        value: termy_core::TerminalGlyphPoint,
    ) -> gpui::Point<Pixels> {
        point(
            bounds.origin.x + bounds.size.width * value.x,
            bounds.origin.y + bounds.size.height * value.y,
        )
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
        assert_eq!(geometry.rects().len(), 8);
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
                terminal_glyph_plan(
                    glyph,
                    test_glyph_metrics(10.0, 20.0, 14.0),
                    TerminalGlyphNeighbors::default(),
                )
                .is_some(),
                "unexpected box-drawing coverage for U+{codepoint:04X}"
            );
        }
    }

    #[test]
    fn upper_half_block_geometry_covers_top_half() {
        let geometry = block_element_geometry('\u{2580}').expect("expected block geometry");
        assert_eq!(geometry.rects().len(), 1);
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
                test_cell(0, '\u{28FF}'),
                test_cell(1, '\u{28FF}'),
                test_cell(2, '\u{28FF}'),
                test_cell(3, 'x'),
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
                test_cell(0, '\u{2830}'),
                test_cell(1, '\u{2830}'),
                test_cell(2, 'x'),
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

        assert_eq!(geometry.rects().len(), 1);
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

        assert_eq!(geometry.rects().len(), 2);
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

        assert_eq!(geometry.rects().len(), 8);

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

        assert_eq!(geometry.rects().len(), 2);

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

        assert_eq!(geometry.rects().len(), 3);

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
    fn rounded_top_left_corner_overlaps_both_neighboring_cells() {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: Size {
                width: px(10.0),
                height: px(20.0),
            },
        };
        let (bounds, plan) = glyph_plan_for_bounds(bounds, '\u{256D}');
        let points = plan.strokes()[0].points();
        let start = resolve_glyph_point(bounds, points[0]);
        let end = resolve_glyph_point(bounds, points[5]);

        assert_f32_eq(start.x.into(), 5.5);
        assert_f32_eq(start.y.into(), 20.5);
        assert_f32_eq(end.x.into(), 10.5);
        assert_f32_eq(end.y.into(), 10.5);
    }

    #[test]
    fn rounded_top_right_corner_overlaps_both_neighboring_cells() {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: Size {
                width: px(10.0),
                height: px(20.0),
            },
        };
        let (bounds, plan) = glyph_plan_for_bounds(bounds, '\u{256E}');
        let points = plan.strokes()[0].points();
        let start = resolve_glyph_point(bounds, points[0]);
        let end = resolve_glyph_point(bounds, points[5]);

        assert_f32_eq(start.x.into(), 5.5);
        assert_f32_eq(start.y.into(), 20.5);
        assert_f32_eq(end.x.into(), -0.5);
        assert_f32_eq(end.y.into(), 10.5);
    }

    #[test]
    fn rounded_bottom_right_corner_overlaps_both_neighboring_cells() {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: Size {
                width: px(20.0),
                height: px(10.0),
            },
        };
        let (bounds, plan) = glyph_plan_for_bounds(bounds, '\u{256F}');
        let points = plan.strokes()[0].points();
        let start = resolve_glyph_point(bounds, points[0]);
        let end = resolve_glyph_point(bounds, points[5]);

        assert_f32_eq(start.x.into(), 10.5);
        assert_f32_eq(start.y.into(), -0.5);
        assert_f32_eq(end.x.into(), -0.5);
        assert_f32_eq(end.y.into(), 5.5);
    }

    #[test]
    fn rounded_bottom_left_corner_overlaps_both_neighboring_cells() {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: Size {
                width: px(20.0),
                height: px(10.0),
            },
        };
        let (bounds, plan) = glyph_plan_for_bounds(bounds, '\u{2570}');
        let points = plan.strokes()[0].points();
        let start = resolve_glyph_point(bounds, points[0]);
        let end = resolve_glyph_point(bounds, points[5]);

        assert_f32_eq(start.x.into(), 10.5);
        assert_f32_eq(start.y.into(), -0.5);
        assert_f32_eq(end.x.into(), 20.5);
        assert_f32_eq(end.y.into(), 5.5);
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
        let (bounds, plan) = glyph_plan_for_bounds(bounds, '\u{2571}');
        assert_eq!(plan.strokes().len(), 1);
        let points = plan.strokes()[0].points();
        let start = resolve_glyph_point(bounds, points[0]);
        let end = resolve_glyph_point(bounds, points[1]);

        assert_f32_eq(start.x.into(), 10.25);
        assert_f32_eq(start.y.into(), -0.5);
        assert_f32_eq(end.x.into(), -0.25);
        assert_f32_eq(end.y.into(), 20.5);
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
        let (bounds, plan) = glyph_plan_for_bounds(bounds, '\u{2573}');
        assert_eq!(plan.strokes().len(), 2);
        let primary = plan.strokes()[0].points();
        let secondary = plan.strokes()[1].points();
        let primary_start = resolve_glyph_point(bounds, primary[0]);
        let primary_end = resolve_glyph_point(bounds, primary[1]);
        let secondary_start = resolve_glyph_point(bounds, secondary[0]);
        let secondary_end = resolve_glyph_point(bounds, secondary[1]);

        assert_f32_eq(primary_start.x.into(), 10.25);
        assert_f32_eq(primary_start.y.into(), -0.5);
        assert_f32_eq(primary_end.x.into(), -0.25);
        assert_f32_eq(primary_end.y.into(), 20.5);

        assert_f32_eq(secondary_start.x.into(), -0.25);
        assert_f32_eq(secondary_start.y.into(), -0.5);
        assert_f32_eq(secondary_end.x.into(), 10.25);
        assert_f32_eq(secondary_end.y.into(), 20.5);
    }

    #[test]
    fn batches_merge_adjacent_cells_with_same_style() {
        let grid = test_grid(vec![test_cell(0, 'a'), test_cell(1, 'b')], None);
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].row, 0);
        assert_eq!(batches[0].start_col, 0);
        assert_eq!(batches[0].text, "ab");
    }

    #[test]
    fn batches_split_on_row_change() {
        let grid = test_grid_rows(vec![vec![test_cell(0, 'a')], vec![test_cell(0, 'b')]], None);
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].text, "a");
        assert_eq!(batches[1].text, "b");
        assert_eq!(batches[0].row, 0);
        assert_eq!(batches[1].row, 1);
    }

    #[test]
    fn batches_split_on_bold_or_color_change() {
        let first = test_cell(0, 'a');
        let mut second = test_cell(1, 'b');
        let mut third = test_cell(2, 'c');
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
    fn batches_preserve_sgr_text_attributes() {
        let mut italic = test_cell(0, 'i');
        let mut underlined = test_cell(1, 'u');
        let mut struck = test_cell(2, 's');
        italic.italic = true;
        underlined.underline = Some(TerminalUnderline {
            style: TerminalUnderlineStyle::Single,
            color: None,
        });
        struck.strikethrough = true;

        let grid = test_grid(vec![italic, underlined, struck], None);
        let batches = collect_batches(&grid);

        assert_eq!(batches.len(), 3);
        assert!(batches[0].italic);
        assert!(batches[0].underline.is_none());
        assert!(!batches[0].strikethrough);
        assert!(!batches[1].italic);
        assert!(batches[1].underline.is_some());
        assert!(!batches[1].strikethrough);
        assert!(!batches[2].italic);
        assert!(batches[2].underline.is_none());
        assert!(batches[2].strikethrough);
    }

    #[test]
    fn terminal_underlines_map_single_and_curly_to_gpui_decorations() {
        let foreground = test_color(0.2, 0.4, 0.6);
        let explicit = test_color(0.8, 0.5, 0.3);

        let single = gpui_underline_style(
            TerminalUnderline {
                style: TerminalUnderlineStyle::Single,
                color: None,
            },
            foreground,
        )
        .expect("single underline should use GPUI decoration");
        assert_eq!(single.thickness, px(1.0));
        assert_eq!(single.color, Some(foreground));
        assert!(!single.wavy);

        let curly = gpui_underline_style(
            TerminalUnderline {
                style: TerminalUnderlineStyle::Curly,
                color: Some(explicit),
            },
            foreground,
        )
        .expect("curly underline should use GPUI decoration");
        assert_eq!(curly.thickness, px(1.0));
        assert_eq!(curly.color, Some(explicit));
        assert!(curly.wavy);

        for style in [
            TerminalUnderlineStyle::Double,
            TerminalUnderlineStyle::Dotted,
            TerminalUnderlineStyle::Dashed,
        ] {
            assert!(
                gpui_underline_style(TerminalUnderline { style, color: None }, foreground)
                    .is_none(),
                "{style:?} must use the custom painter"
            );
        }
    }

    #[test]
    fn custom_underline_geometry_is_pixel_snapped_and_bounded() {
        let bounds = Bounds {
            origin: point(px(12.3), px(40.7)),
            size: Size {
                width: px(20.2),
                height: px(10.2),
            },
        };

        let underline_origin_y = px(49.0);
        let double =
            custom_underline_path_spec(bounds, underline_origin_y, TerminalUnderlineStyle::Double)
                .expect("double underline geometry");
        assert_f32_eq(double.start_x, 12.5);
        assert_f32_eq(double.end_x, 32.5);
        assert_eq!(double.line_count, 2);
        assert_f32_eq(double.line_y[0], 47.5);
        assert_f32_eq(double.line_y[1], 49.5);
        assert_eq!(double.pattern, CustomUnderlinePattern::Solid);

        let dotted =
            custom_underline_path_spec(bounds, underline_origin_y, TerminalUnderlineStyle::Dotted)
                .expect("dotted underline geometry");
        assert_eq!(dotted.line_count, 1);
        assert_f32_eq(dotted.line_y[0], 49.5);
        assert_eq!(dotted.pattern, CustomUnderlinePattern::Dotted);

        let dashed =
            custom_underline_path_spec(bounds, underline_origin_y, TerminalUnderlineStyle::Dashed)
                .expect("dashed underline geometry");
        assert_eq!(dashed.line_count, 1);
        assert_eq!(dashed.pattern, CustomUnderlinePattern::Dashed);

        for spec in [double, dotted, dashed] {
            assert!(spec.start_x >= 12.0);
            assert!(spec.end_x <= 33.0);
            assert!(
                spec.line_y[..spec.line_count]
                    .iter()
                    .all(|y| *y >= 41.0 && *y <= 51.0)
            );
        }
        assert!(
            custom_underline_path_spec(bounds, underline_origin_y, TerminalUnderlineStyle::Single,)
                .is_none()
        );
        assert!(
            custom_underline_path_spec(bounds, underline_origin_y, TerminalUnderlineStyle::Curly,)
                .is_none()
        );
    }

    #[test]
    fn batches_split_on_underline_style_and_color() {
        let explicit = test_color(0.9, 0.4, 0.2);
        let mut single = test_cell(0, 'a');
        let mut double = test_cell(1, 'b');
        let mut colored = test_cell(2, 'c');
        let mut same_colored = test_cell(3, 'd');
        single.underline = Some(TerminalUnderline {
            style: TerminalUnderlineStyle::Single,
            color: None,
        });
        double.underline = Some(TerminalUnderline {
            style: TerminalUnderlineStyle::Double,
            color: None,
        });
        colored.underline = Some(TerminalUnderline {
            style: TerminalUnderlineStyle::Double,
            color: Some(explicit),
        });
        same_colored.underline = colored.underline;

        let batches = collect_batches(&test_grid(
            vec![single, double, colored, same_colored],
            None,
        ));
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].text, "a");
        assert_eq!(batches[1].text, "b");
        assert_eq!(batches[2].text, "cd");
        assert_eq!(
            batches[0].underline,
            Some(TerminalUnderline {
                style: TerminalUnderlineStyle::Single,
                color: None,
            })
        );
        assert_eq!(
            batches[1].underline,
            Some(TerminalUnderline {
                style: TerminalUnderlineStyle::Double,
                color: None,
            })
        );
        assert_eq!(
            batches[2].underline,
            Some(TerminalUnderline {
                style: TerminalUnderlineStyle::Double,
                color: Some(explicit),
            })
        );
    }

    #[test]
    fn cached_text_batch_equality_rejects_stale_underline_decoration() {
        let key = TextBatchKey {
            bold: false,
            italic: false,
            strikethrough: false,
            fg: test_color(0.4, 0.5, 0.6),
        };
        let make_batch =
            |underline| TextBatchBuilder::new(0, 0, 'x', None, key, Some(underline)).finalize();
        let single = make_batch(TerminalUnderline {
            style: TerminalUnderlineStyle::Single,
            color: None,
        });
        let curly = make_batch(TerminalUnderline {
            style: TerminalUnderlineStyle::Curly,
            color: None,
        });
        let colored = make_batch(TerminalUnderline {
            style: TerminalUnderlineStyle::Single,
            color: Some(test_color(0.7, 0.2, 0.3)),
        });

        assert!(!text_batches_match_without_row(&single, &curly));
        assert!(!text_batches_match_without_row(&single, &colored));
        assert!(text_batches_match_without_row(&single, &single.clone()));
    }

    #[test]
    fn batches_split_on_hover_underline_boundary() {
        let grid = test_grid(
            vec![test_cell(0, 'a'), test_cell(1, 'b'), test_cell(2, 'c')],
            Some((0, 1, 0, 2)),
        );
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].text, "a");
        assert!(batches[0].underline.is_none());
        assert_eq!(batches[1].text, "bc");
        assert!(batches[1].underline.is_some());
    }

    #[test]
    fn hover_underline_covers_each_row_of_wrapped_link() {
        let grid = test_grid_rows(
            vec![
                vec![test_cell(0, 'a'), test_cell(1, 'b')],
                vec![test_cell(0, 'c'), test_cell(1, 'd')],
                vec![test_cell(0, 'e'), test_cell(1, 'f')],
            ],
            Some((0, 1, 2, 0)),
        );
        let color = test_color(1.0, 1.0, 1.0);

        assert!(grid.cell_underline(0, 0, color, None).is_none());
        assert!(grid.cell_underline(0, 1, color, None).is_some());
        assert!(grid.cell_underline(1, 0, color, None).is_some());
        assert!(grid.cell_underline(1, 1, color, None).is_some());
        assert!(grid.cell_underline(2, 0, color, None).is_some());
        assert!(grid.cell_underline(2, 1, color, None).is_none());
        assert!(
            grid.cell_underline(
                2,
                1,
                color,
                Some(TerminalUnderline {
                    style: TerminalUnderlineStyle::Single,
                    color: None,
                })
            )
            .is_some()
        );
    }

    #[test]
    fn batches_keep_emoji_in_normal_text_flow() {
        let grid = test_grid(
            vec![test_cell(0, 'a'), test_cell(1, '📦'), test_cell(2, 'b')],
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
            vec![test_cell(0, 'a'), test_cell(1, '📦'), test_cell(2, 'b')],
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
        let mut spacer = test_cell(1, 'x');
        spacer.render_text = false;
        let mut control = test_cell(2, '\u{001B}');
        control.render_text = true;
        let grid = test_grid(
            vec![
                test_cell(0, 'a'),
                spacer,
                control,
                test_cell(3, ' '),
                test_cell(4, '\0'),
                test_cell(5, 'b'),
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
                test_cell(0, 'a'),
                test_cell(1, '\u{2588}'),
                test_cell(2, 'b'),
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
        let mut spacer = test_cell(1, ' ');
        spacer.render_text = false;
        let grid = test_grid(vec![test_cell(0, '你'), spacer, test_cell(2, 'x')], None);
        let batches = collect_batches(&grid);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].text, "你");
        assert_eq!(batches[1].text, "x");
    }

    #[test]
    fn draw_ops_interleave_text_and_block_in_cell_order() {
        let grid = test_grid(
            vec![
                test_cell(0, 'a'),
                test_cell(1, '\u{2588}'),
                test_cell(2, 'b'),
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
                test_cell(0, 'a'),
                test_cell(1, 'b'),
                test_cell(2, '\u{2588}'),
                test_cell(3, 'c'),
                test_cell(4, 'd'),
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
                test_cell(0, 'a'),
                test_cell(1, 'b'),
                test_cell(2, '\u{2502}'),
                test_cell(3, 'c'),
                test_cell(4, 'd'),
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
                test_cell(0, 'a'),
                test_cell(1, '\u{256D}'),
                test_cell(2, 'b'),
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
                test_cell(0, 'a'),
                test_cell(1, '\u{2573}'),
                test_cell(2, 'b'),
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
        let mut spacer = test_cell(1, 'x');
        spacer.render_text = false;
        let mut control = test_cell(3, '\u{001B}');
        control.render_text = true;
        let grid = test_grid(
            vec![
                test_cell(0, 'a'),
                spacer,
                test_cell(2, '\u{2588}'),
                control,
                test_cell(4, 'b'),
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
        let grid = test_grid_rows(
            vec![
                vec![test_cell(0, 'a'), test_cell(1, 'b')],
                vec![
                    test_cell(0, 'c'),
                    test_cell(1, '\u{2588}'),
                    test_cell(2, 'd'),
                ],
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
    fn moved_cached_row_uses_outer_row_for_cursor_and_hover() {
        let cached_row = Arc::new(vec![test_cell(0, 'x')]);
        let empty_row = Arc::new(Vec::new());
        let mut grid = test_grid_rows(vec![Vec::new(), Vec::new()], Some((0, 0, 0, 0)));
        grid.cols = 1;
        grid.cells = Arc::new(vec![Arc::clone(&cached_row), Arc::clone(&empty_row)]);
        grid.cursor_cell = Some((0, 0));
        grid.cursor_visible = true;
        let cursor_fg = test_color(0.7, 0.6, 0.5);
        let highlight_fg = test_color(0.2, 0.3, 0.4);

        let original_ops = grid.collect_draw_ops(cursor_fg, highlight_fg);
        assert!(matches!(
            &original_ops[..],
            [TextDrawOp::Batch(batch)]
                if batch.row == 0 && batch.fg == cursor_fg && batch.underline.is_some()
        ));

        grid.cells = Arc::new(vec![empty_row, cached_row]);
        grid.cursor_cell = Some((0, 1));
        grid.hovered_link_range = Some((1, 0, 1, 0));

        let moved_ops = grid.collect_draw_ops(cursor_fg, highlight_fg);
        assert!(matches!(
            &moved_ops[..],
            [TextDrawOp::Batch(batch)]
                if batch.row == 1 && batch.fg == cursor_fg && batch.underline.is_some()
        ));
    }

    #[test]
    fn block_draw_uses_same_fg_precedence_as_text() {
        let mut selected_text = test_cell(0, 'x');
        selected_text.selected = true;
        let mut selected_block = test_cell(1, '\u{2588}');
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

        let mut cursor_block = test_cell(0, '\u{2588}');
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
        let mut grid = test_grid(vec![test_cell(0, 'a')], None);
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
        let mut grid = test_grid(vec![test_cell(0, 'a')], None);
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
        let mut grid = test_grid(vec![test_cell(0, 'a')], None);
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
        let mut grid = test_grid(vec![test_cell(0, 'a')], Some((3, 1, 3, 2)));
        grid.rows = 5;
        grid.paint_damage = TerminalGridPaintDamage::None;
        let mut cache = TerminalGridPaintCache {
            style_key: Some(grid.paint_style_key()),
            last_hovered_link_range: Some((1, 0, 1, 0)),
            ..Default::default()
        };
        let (full, style_changed, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);
        assert!(!full);
        assert!(!style_changed);
        assert_eq!(&*dirty_rows, &[1usize, 3usize]);
    }

    #[test]
    fn dirty_rows_for_pass_includes_every_wrapped_link_row() {
        let mut grid = test_grid(vec![test_cell(0, 'a')], Some((2, 1, 4, 3)));
        grid.rows = 6;
        grid.paint_damage = TerminalGridPaintDamage::None;
        let mut cache = TerminalGridPaintCache {
            style_key: Some(grid.paint_style_key()),
            last_hovered_link_range: Some((0, 0, 1, 2)),
            ..Default::default()
        };

        let (full, style_changed, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);

        assert!(!full);
        assert!(!style_changed);
        assert_eq!(&*dirty_rows, &[0usize, 1usize, 2usize, 3usize, 4usize]);
    }

    #[test]
    fn dirty_rows_for_pass_forces_full_repaint_when_style_changes() {
        let grid = test_grid(vec![test_cell(0, 'a')], None);
        let mut cache = TerminalGridPaintCache::default();
        let (full, style_changed, dirty_rows) = grid.dirty_rows_for_pass(&mut cache);
        assert!(full);
        assert!(style_changed);
        assert!(dirty_rows.is_empty());
    }

    #[test]
    fn row_background_spans_merge_contiguous_cells_with_same_fill() {
        let mut first = test_cell(0, 'a');
        let mut second = test_cell(1, 'b');
        let mut third = test_cell(2, 'c');
        let mut fourth = test_cell(3, 'd');
        let mut fifth = test_cell(4, 'e');
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
        let mut default_bg_cell = test_cell(0, 'a');
        let mut ansi_bg_cell = test_cell(1, 'b');
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
        let mut default_bg_cell = test_cell(0, 'a');
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
        let mut half_block = test_cell(0, '\u{2580}');
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
        let old_grid = test_grid_rows(vec![vec![test_cell(0, 'a')], vec![test_cell(0, 'b')]], None);
        let new_grid = test_grid_rows(vec![vec![test_cell(0, 'b')], vec![test_cell(0, 'c')]], None);
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
                0,
                old_grid.cells[0].as_slice(),
                cursor_fg,
                highlight_fg,
                &mut HashMap::new(),
            ),
            old_grid.rebuild_cached_row_ops(
                1,
                old_grid.cells[1].as_slice(),
                cursor_fg,
                highlight_fg,
                &mut HashMap::new(),
            ),
        ];
        let next_row_ops = new_grid.rebuild_cached_row_ops(
            0,
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
        let previous_grid = test_grid(vec![test_cell(0, 'a')], Some((0, 0, 0, 0)));
        let next_grid = test_grid(vec![test_cell(0, 'a')], None);
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
            0,
            previous_grid.cells[0].as_slice(),
            cursor_fg,
            highlight_fg,
            &mut HashMap::new(),
        )];
        let next_row_ops = next_grid.rebuild_cached_row_ops(
            0,
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
                test_cell(0, 'a'),
                test_cell(1, '\u{2588}'),
                test_cell(2, 'b'),
            ],
            None,
        );
        let row_ops = grid.rebuild_cached_row_ops(
            0,
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
        let mut grid = test_grid(vec![test_cell(0, 'a')], None);
        grid.rows = 2;
        grid.cells = Arc::new(vec![Arc::new(vec![test_cell(0, 'a')])]);

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

        let stale_row_cells = vec![test_cell(0, 'z')];
        let mut cache = TerminalGridPaintCache {
            row_ops: vec![
                CachedRowPaintOps::default(),
                grid.rebuild_cached_row_ops(
                    1,
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
        let mut grid = test_grid(vec![test_cell(0, 'a')], None);
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
        let mut grid = test_grid(vec![test_cell(0, 'a')], None);
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
                None,
                TextBatchKey {
                    bold: false,
                    italic: false,
                    strikethrough: false,
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
    fn text_batches_preserve_combining_text_without_consuming_columns() {
        let key = TextBatchKey {
            bold: false,
            italic: false,
            strikethrough: false,
            fg: Hsla::transparent_black(),
        };
        let mut builder = TextBatchBuilder::new(0, 0, 'e', Some("\u{301}"), key, None);
        builder.append_cell('x', Some("\u{308}"));
        let batch = builder.finalize();

        assert_eq!(batch.text, "e\u{301}x\u{308}");
        assert_eq!(batch.cell_len, 2);
    }

    #[test]
    fn draw_ops_shape_combining_text_with_its_base_cell() {
        let mut cell = test_cell(0, 'e');
        cell.combining = Some(SharedString::from("\u{301}"));
        let grid = test_grid(vec![cell], None);
        let ops = collect_draw_ops(&grid);

        assert!(
            matches!(&ops[..], [TextDrawOp::Batch(batch)] if batch.text == "e\u{301}" && batch.cell_len == 1)
        );
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
            geometry: sextant_geometry('\u{1FB00}').unwrap(),
            fg: Hsla::transparent_black(),
        });
        assert_eq!(draw_op_col_range(&sextant), (4, 4));
    }

    #[test]
    fn sextant_geometry_matches_terminal_qr_decoding() {
        let first = sextant_geometry('\u{1FB00}').expect("first sextant");
        assert_eq!(first.rects().len(), 1);
        assert_eq!(first.rects()[0].left, 0.0);
        assert_eq!(first.rects()[0].top, 0.0);
        assert_eq!(first.rects()[0].right, 0.5);
        assert_eq!(first.rects()[0].bottom, 1.0 / 3.0);
        assert_eq!(first.rects()[0].snap, TerminalGlyphRectSnap::Outward);
        assert_eq!(sextant_geometry('\u{1FB3B}').unwrap().rects().len(), 5);
        assert!(sextant_geometry('█').is_none());
        assert!(sextant_geometry('\u{1FAFF}').is_none());
        assert!(sextant_geometry('\u{1FB3C}').is_none());
    }

    #[test]
    fn draw_ops_emit_sextant_for_legacy_mosaic() {
        let grid = test_grid(vec![test_cell(0, 'a'), test_cell(1, '\u{1FB00}')], None);
        let ops = collect_draw_ops(&grid);
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], TextDrawOp::Batch(batch) if batch.text == "a"));
        assert!(
            matches!(&ops[1], TextDrawOp::Sextant(s) if s.row == 0 && s.col == 1 && s.geometry.rects().len() == 1)
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
        let mut grid = test_grid(vec![test_cell(0, 'a')], None);
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
