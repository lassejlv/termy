#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

mod box_drawing;
mod braille;
mod instance_rows;
mod palette;
mod rects;
mod rounded_corners;
mod text_rows;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use box_drawing::{BoxDrawing, BoxMetrics, box_drawing, box_rectangles};
use braille::{BrailleInstance, BrailleRenderer};
use engine::{
    Cell, CellFlags, CursorShape, CursorState, DynamicColor, FrameUpdate, RowMove,
    RowMoveDirection, SelectionRange,
};
use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, Style, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use instance_rows::{RowTransforms, UploadStats};
pub use palette::Theme;
use palette::{Palette, srgb_to_linear};
use rects::{RectInstance, RectRenderer};
use rounded_corners::{RoundedCornerInstance, RoundedCornerRenderer};
use text_rows::TextRows;
use wgpu::{
    Backends, CommandEncoderDescriptor, CompositeAlphaMode, DeviceDescriptor, Instance,
    InstanceDescriptor, LoadOp, Operations, PresentMode, RenderPassColorAttachment,
    RenderPassDescriptor, RequestAdapterOptions, SurfaceColorSpace, SurfaceConfiguration,
    TextureFormat, TextureUsages, TextureViewDescriptor,
};
use winit::{dpi::PhysicalSize, event_loop::ActiveEventLoop, window::Window};

const DEFAULT_FONT_FAMILY: &str = "Menlo";
const ATLAS_TRIM_INTERVAL: u64 = 120;
const SEARCH_FIELD_WIDTH: f32 = 280.0;
const SEARCH_FIELD_HEIGHT: f32 = 34.0;
const SEARCH_FIELD_MARGIN: f32 = 12.0;
const SEARCH_FIELD_INSET: f32 = 11.0;
const SEARCH_FONT_SIZE: f32 = 13.0;
const SEARCH_LINE_HEIGHT: f32 = 18.0;

#[derive(Clone, Debug)]
pub struct RendererConfig {
    pub font_family: String,
    pub font_size: f32,
    pub padding: f32,
    pub theme: Theme,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            font_family: DEFAULT_FONT_FAMILY.to_owned(),
            font_size: 15.0,
            padding: 8.0,
            theme: Theme::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchInput {
    pub query: String,
    pub has_match: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderStatus {
    Presented,
    Retry,
    Occluded,
}

/// Lightweight counters for validating that retained rendering stays on its partial path.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RendererMetrics {
    pub frame_updates: u64,
    pub acquired_frames: u64,
    pub surface_retries: u64,
    pub occluded_frames: u64,
    pub rebuilt_rows: u64,
    pub row_rectangle_plans_rebuilt: u64,
    pub viewport_updates: u64,
    pub text_prepares: u64,
    pub text_row_prepares: u64,
    pub text_rows_reused: u64,
    pub ascii_rows_shaped: u64,
    pub complex_rows_shaped: u64,
    pub rectangle_builds: u64,
    pub rectangle_uploads: u64,
    pub rectangle_instances_uploaded: u64,
    pub rectangle_scratch_growths: u64,
    pub rounded_corner_builds: u64,
    pub rounded_corner_uploads: u64,
    pub rounded_corner_instances_uploaded: u64,
    pub rounded_corner_scratch_growths: u64,
    pub braille_builds: u64,
    pub braille_uploads: u64,
    pub braille_instances_uploaded: u64,
    pub braille_scratch_growths: u64,
    pub upload_bytes: u64,
    pub coalesced_updates: u64,
    pub skipped_frames: u64,
    pub static_geometry_builds: u64,
    pub static_geometry_writes: u64,
    pub static_geometry_bytes: u64,
    pub static_rows_reused: u64,
    pub dynamic_geometry_builds: u64,
    pub dynamic_geometry_writes: u64,
    pub dynamic_geometry_bytes: u64,
    pub geometry_buffer_growths: u64,
    pub transform_writes: u64,
    pub transform_bytes: u64,
    pub atlas_trims: u64,
    pub missed_refresh_deadlines: u64,
    pub presented_frames: u64,
}

/// Opt-in timings for one frame. All values are monotonic elapsed nanoseconds.
///
/// Collection is disabled by default so the ordinary render path does not read a clock.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RendererFrameTimings {
    pub apply_frame_ns: u64,
    pub surface_acquire_ns: u64,
    pub viewport_update_ns: u64,
    pub glyph_prepare_ns: u64,
    pub geometry_build_ns: u64,
    pub geometry_upload_ns: u64,
    pub encoding_ns: u64,
    pub submission_ns: u64,
    pub presentation_ns: u64,
    pub render_total_ns: u64,
    pub end_to_end_ns: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct MeasurementState {
    enabled: bool,
    frame_budget_ns: Option<u64>,
    pending_apply_ns: u64,
    last_frame: Option<RendererFrameTimings>,
}

#[derive(Debug, Default)]
struct GeometryState {
    static_rows: Vec<bool>,
    dynamic: bool,
    transforms: bool,
}

#[derive(Debug)]
struct RenderRow {
    buffer: Buffer,
    text: String,
    styles: Vec<TextRun>,
    wide_glyphs: Vec<WideGlyph>,
    backgrounds: Vec<BackgroundRun>,
    blocks: Vec<BlockPrimitive>,
    boxes: Vec<BoxPrimitive>,
    braille: Vec<BraillePrimitive>,
    decorations: Vec<DecorationPrimitive>,
    shaping: Shaping,
    generation: u64,
}

#[derive(Debug)]
struct WideGlyph {
    buffer: Buffer,
    column: usize,
    columns: usize,
    text: String,
    style: TextStyle,
}

#[derive(Debug)]
struct RetainedGrid {
    row_plans: Vec<RenderRow>,
    cells: Vec<Vec<Cell>>,
    columns: usize,
    rows: usize,
    display_offset: usize,
    font_family: String,
    next_generation: u64,
}

impl Default for RetainedGrid {
    fn default() -> Self {
        Self {
            row_plans: Vec::new(),
            cells: Vec::new(),
            columns: 0,
            rows: 0,
            display_offset: 0,
            font_family: DEFAULT_FONT_FAMILY.to_owned(),
            next_generation: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextStyle {
    foreground: [u8; 3],
    bold: bool,
    italic: bool,
}

#[derive(Clone, Copy, Debug)]
struct TextRun {
    start: usize,
    end: usize,
    style: TextStyle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BackgroundRun {
    start_column: usize,
    end_column: usize,
    color: [u8; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DecorationPrimitive {
    column: usize,
    kind: DecorationKind,
    foreground: [u8; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecorationKind {
    Underline,
    Strikeout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockPrimitive {
    column: usize,
    shape: BlockShape,
    foreground: [u8; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BoxPrimitive {
    column: usize,
    shape: BoxDrawing,
    foreground: [u8; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BraillePrimitive {
    column: usize,
    pattern: u8,
    foreground: [u8; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockShape {
    Rectangle(EighthRect),
    Quadrants(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EighthRect {
    left: u8,
    top: u8,
    right: u8,
    bottom: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PixelRect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RetainedUpdate {
    rebuilt_rows: usize,
    dimensions_changed: bool,
    rows_repositioned: bool,
    ascii_rows_shaped: usize,
    complex_rows_shaped: usize,
}

impl RetainedUpdate {
    const fn text_changed(self) -> bool {
        self.rebuilt_rows != 0 || self.dimensions_changed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RenderPreparationState {
    text_dirty: bool,
    viewport_dirty: bool,
}

impl Default for RenderPreparationState {
    fn default() -> Self {
        Self {
            text_dirty: true,
            viewport_dirty: true,
        }
    }
}

impl RenderPreparationState {
    const fn text_is_dirty(self) -> bool {
        self.text_dirty
    }

    const fn mark_text_dirty(&mut self) {
        self.text_dirty = true;
    }

    const fn viewport_is_dirty(self) -> bool {
        self.viewport_dirty
    }

    const fn mark_viewport_dirty(&mut self) {
        self.viewport_dirty = true;
    }

    const fn viewport_updated(&mut self) {
        self.viewport_dirty = false;
    }

    const fn text_prepared(&mut self) {
        self.text_dirty = false;
    }
}

#[derive(Clone, Copy, Debug)]
struct RectangleFrame {
    selection: Option<SelectionRange>,
    cursor: CursorState,
    cursor_blink_visible: bool,
    padding: f32,
    cell_metrics: CellMetrics,
    font_size: f32,
    viewport_width: f32,
    viewport_height: f32,
    cursor_color: [u8; 3],
    selection_color: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SearchFieldLayout {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    text_x: f32,
    text_y: f32,
    text_width: f32,
}

impl EighthRect {
    const fn new(left: u8, top: u8, right: u8, bottom: u8) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
}

impl BlockShape {
    const UPPER_LEFT: u8 = 1 << 0;
    const UPPER_RIGHT: u8 = 1 << 1;
    const LOWER_LEFT: u8 = 1 << 2;
    const LOWER_RIGHT: u8 = 1 << 3;

    const fn rectangle(left: u8, top: u8, right: u8, bottom: u8) -> Self {
        Self::Rectangle(EighthRect::new(left, top, right, bottom))
    }

    const fn from_char(character: char) -> Option<Self> {
        match character {
            '\u{2580}' => Some(Self::rectangle(0, 0, 8, 4)),
            '\u{2581}' => Some(Self::rectangle(0, 7, 8, 8)),
            '\u{2582}' => Some(Self::rectangle(0, 6, 8, 8)),
            '\u{2583}' => Some(Self::rectangle(0, 5, 8, 8)),
            '\u{2584}' => Some(Self::rectangle(0, 4, 8, 8)),
            '\u{2585}' => Some(Self::rectangle(0, 3, 8, 8)),
            '\u{2586}' => Some(Self::rectangle(0, 2, 8, 8)),
            '\u{2587}' => Some(Self::rectangle(0, 1, 8, 8)),
            '\u{2588}' => Some(Self::rectangle(0, 0, 8, 8)),
            '\u{2589}' => Some(Self::rectangle(0, 0, 7, 8)),
            '\u{258A}' => Some(Self::rectangle(0, 0, 6, 8)),
            '\u{258B}' => Some(Self::rectangle(0, 0, 5, 8)),
            '\u{258C}' => Some(Self::rectangle(0, 0, 4, 8)),
            '\u{258D}' => Some(Self::rectangle(0, 0, 3, 8)),
            '\u{258E}' => Some(Self::rectangle(0, 0, 2, 8)),
            '\u{258F}' => Some(Self::rectangle(0, 0, 1, 8)),
            '\u{2590}' => Some(Self::rectangle(4, 0, 8, 8)),
            '\u{2594}' => Some(Self::rectangle(0, 0, 8, 1)),
            '\u{2595}' => Some(Self::rectangle(7, 0, 8, 8)),
            '\u{2596}' => Some(Self::Quadrants(Self::LOWER_LEFT)),
            '\u{2597}' => Some(Self::Quadrants(Self::LOWER_RIGHT)),
            '\u{2598}' => Some(Self::Quadrants(Self::UPPER_LEFT)),
            '\u{2599}' => Some(Self::Quadrants(
                Self::UPPER_LEFT | Self::LOWER_LEFT | Self::LOWER_RIGHT,
            )),
            '\u{259A}' => Some(Self::Quadrants(Self::UPPER_LEFT | Self::LOWER_RIGHT)),
            '\u{259B}' => Some(Self::Quadrants(
                Self::UPPER_LEFT | Self::UPPER_RIGHT | Self::LOWER_LEFT,
            )),
            '\u{259C}' => Some(Self::Quadrants(
                Self::UPPER_LEFT | Self::UPPER_RIGHT | Self::LOWER_RIGHT,
            )),
            '\u{259D}' => Some(Self::Quadrants(Self::UPPER_RIGHT)),
            '\u{259E}' => Some(Self::Quadrants(Self::UPPER_RIGHT | Self::LOWER_LEFT)),
            '\u{259F}' => Some(Self::Quadrants(
                Self::UPPER_RIGHT | Self::LOWER_LEFT | Self::LOWER_RIGHT,
            )),
            _ => None,
        }
    }

    const fn eighth_rectangles(self) -> [Option<EighthRect>; 4] {
        match self {
            Self::Rectangle(rectangle) => [Some(rectangle), None, None, None],
            Self::Quadrants(mask) => [
                if mask & Self::UPPER_LEFT != 0 {
                    Some(EighthRect::new(0, 0, 4, 4))
                } else {
                    None
                },
                if mask & Self::UPPER_RIGHT != 0 {
                    Some(EighthRect::new(4, 0, 8, 4))
                } else {
                    None
                },
                if mask & Self::LOWER_LEFT != 0 {
                    Some(EighthRect::new(0, 4, 4, 8))
                } else {
                    None
                },
                if mask & Self::LOWER_RIGHT != 0 {
                    Some(EighthRect::new(4, 4, 8, 8))
                } else {
                    None
                },
            ],
        }
    }
}

fn geometric_block_shape(text: &str) -> Option<BlockShape> {
    let mut characters = text.chars();
    let shape = BlockShape::from_char(characters.next()?)?;
    characters.next().is_none().then_some(shape)
}

fn geometric_box_shape(text: &str) -> Option<BoxDrawing> {
    let mut characters = text.chars();
    let shape = box_drawing(characters.next()?)?;
    characters.next().is_none().then_some(shape)
}

fn geometric_braille_pattern(text: &str) -> Option<u8> {
    let mut characters = text.chars();
    let character = characters.next()?;
    if characters.next().is_some() || !(('\u{2800}'..='\u{28FF}').contains(&character)) {
        return None;
    }
    Some((character as u32 - 0x2800) as u8)
}

fn rounded_cell_rect(
    padding: f32,
    cell_width: f32,
    cell_height: f32,
    column: usize,
    row: usize,
) -> PixelRect {
    PixelRect {
        left: (padding + column as f32 * cell_width).round(),
        top: (padding + row as f32 * cell_height).round(),
        right: (padding + (column + 1) as f32 * cell_width).round(),
        bottom: (padding + (row + 1) as f32 * cell_height).round(),
    }
}

fn block_pixel_rectangles(shape: BlockShape, cell: PixelRect) -> [Option<PixelRect>; 4] {
    shape.eighth_rectangles().map(|rectangle| {
        rectangle.map(|rectangle| {
            let horizontal =
                |eighth| (cell.left + (cell.right - cell.left) * f32::from(eighth) / 8.0).round();
            let vertical =
                |eighth| (cell.top + (cell.bottom - cell.top) * f32::from(eighth) / 8.0).round();
            PixelRect {
                left: horizontal(rectangle.left),
                top: vertical(rectangle.top),
                right: horizontal(rectangle.right),
                bottom: vertical(rectangle.bottom),
            }
        })
    })
}

impl RenderRow {
    fn new(font_system: &mut FontSystem, metrics: Metrics, cell_width: f32) -> Self {
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_wrap(Wrap::None);
        buffer.set_monospace_width(Some(cell_width));
        Self {
            buffer,
            text: String::new(),
            styles: Vec::new(),
            wide_glyphs: Vec::new(),
            backgrounds: Vec::new(),
            blocks: Vec::new(),
            boxes: Vec::new(),
            braille: Vec::new(),
            decorations: Vec::new(),
            shaping: Shaping::Advanced,
            generation: 0,
        }
    }
}

impl WideGlyph {
    fn new(font_system: &mut FontSystem, metrics: Metrics) -> Self {
        let mut buffer = Buffer::new(font_system, metrics);
        buffer.set_wrap(Wrap::None);
        Self {
            buffer,
            column: 0,
            columns: 0,
            text: String::new(),
            style: TextStyle {
                foreground: [0; 3],
                bold: false,
                italic: false,
            },
        }
    }

    fn begin(&mut self, column: usize, text: &str, style: TextStyle) {
        self.column = column;
        self.columns = 2;
        self.text.clear();
        self.text.push_str(text);
        self.style = style;
    }

    fn can_append(&self, column: usize, style: TextStyle) -> bool {
        self.column.saturating_add(self.columns) == column && self.style == style
    }

    fn append(&mut self, text: &str) {
        self.columns = self.columns.saturating_add(2);
        self.text.push_str(text);
    }

    fn shape(
        &mut self,
        font_system: &mut FontSystem,
        metrics: Metrics,
        cell_width: f32,
        font_family: &str,
    ) {
        self.buffer.set_metrics(metrics);
        self.buffer.set_monospace_width(None);
        self.buffer.set_size(
            Some(cell_width * self.columns as f32),
            Some(metrics.line_height),
        );
        self.buffer.set_text(
            &self.text,
            &attrs_for_style(self.style, font_family),
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(font_system, false);
    }
}

impl RetainedGrid {
    fn with_font_family(font_family: String) -> Self {
        Self {
            font_family,
            ..Self::default()
        }
    }

    fn apply_frame(
        &mut self,
        update: &FrameUpdate,
        font_system: &mut FontSystem,
        metrics: Metrics,
        cell_width: f32,
        palette: Palette,
    ) -> RetainedUpdate {
        let dimensions_changed = update.columns != self.columns || update.rows != self.rows;
        let mut rows_repositioned = false;
        let forced_rows = if dimensions_changed {
            Some(self.resize_storage(
                update.columns,
                update.rows,
                font_system,
                metrics,
                cell_width,
            ))
        } else if update.full {
            rows_repositioned = self.rotate_for_display_scroll(update.display_offset);
            None
        } else {
            None
        };

        for movement in &update.row_moves {
            rows_repositioned |= self.apply_row_move(*movement);
        }

        let mut shaped_rows = 0;
        let mut ascii_rows_shaped = 0;
        let mut complex_rows_shaped = 0;
        for row in &update.row_updates {
            let end = row.start_column.saturating_add(row.cells.len());
            if row.index < self.rows && end <= self.columns {
                let cells = &mut self.cells[row.index][row.start_column..end];
                let forced = forced_rows.as_ref().is_some_and(|rows| rows[row.index]);
                if !forced && cells == row.cells.as_slice() {
                    continue;
                }
                cells.clone_from_slice(&row.cells);
                match self.rebuild_row(row.index, font_system, metrics, cell_width, palette) {
                    Shaping::Basic => ascii_rows_shaped += 1,
                    Shaping::Advanced => complex_rows_shaped += 1,
                }
                shaped_rows += 1;
            }
        }
        self.display_offset = update.display_offset;
        RetainedUpdate {
            rebuilt_rows: shaped_rows,
            dimensions_changed,
            rows_repositioned,
            ascii_rows_shaped,
            complex_rows_shaped,
        }
    }

    fn apply_row_move(&mut self, movement: RowMove) -> bool {
        let height = movement.end_row.saturating_sub(movement.start_row);
        if movement.start_row >= movement.end_row
            || movement.end_row > self.rows
            || movement.count == 0
            || movement.count >= height
        {
            return false;
        }
        let cells = &mut self.cells[movement.start_row..movement.end_row];
        let plans = &mut self.row_plans[movement.start_row..movement.end_row];
        match movement.direction {
            RowMoveDirection::Up => {
                cells.rotate_left(movement.count);
                plans.rotate_left(movement.count);
            }
            RowMoveDirection::Down => {
                cells.rotate_right(movement.count);
                plans.rotate_right(movement.count);
            }
        }
        true
    }

    fn resize_storage(
        &mut self,
        columns: usize,
        rows: usize,
        font_system: &mut FontSystem,
        metrics: Metrics,
        cell_width: f32,
    ) -> Vec<bool> {
        let default_cell = Cell::default();
        let mut forced_rows = vec![false; rows];
        if columns != self.columns {
            for (index, row) in self.cells.iter_mut().take(rows).enumerate() {
                if columns < row.len() && row[columns..].iter().any(|cell| cell != &default_cell) {
                    forced_rows[index] = true;
                }
                row.resize_with(columns, Cell::default);
                row.truncate(columns);
            }
        }

        self.cells.truncate(rows);
        self.cells
            .resize_with(rows, || vec![Cell::default(); columns]);
        self.row_plans.truncate(rows);
        while self.row_plans.len() < rows {
            self.row_plans
                .push(RenderRow::new(font_system, metrics, cell_width));
        }

        self.columns = columns;
        self.rows = rows;
        for row in &mut self.row_plans {
            row.buffer
                .set_size(Some(columns as f32 * cell_width), Some(metrics.line_height));
        }
        forced_rows
    }

    fn rotate_for_display_scroll(&mut self, next_display_offset: usize) -> bool {
        if self.rows == 0 || next_display_offset == self.display_offset {
            return false;
        }
        let mut rotated = false;
        if next_display_offset > self.display_offset {
            let lines = next_display_offset - self.display_offset;
            if lines < self.rows {
                self.cells.rotate_right(lines);
                self.row_plans.rotate_right(lines);
                rotated = true;
            }
        } else {
            let lines = self.display_offset - next_display_offset;
            if lines < self.rows {
                self.cells.rotate_left(lines);
                self.row_plans.rotate_left(lines);
                rotated = true;
            }
        }
        rotated
    }

    fn remeasure_and_rebuild(
        &mut self,
        font_system: &mut FontSystem,
        metrics: Metrics,
        cell_width: f32,
        palette: Palette,
    ) -> usize {
        for row in &mut self.row_plans {
            row.buffer.set_metrics(metrics);
            row.buffer.set_monospace_width(Some(cell_width));
        }
        self.rebuild_all(font_system, metrics, cell_width, palette)
    }

    fn rebuild_all(
        &mut self,
        font_system: &mut FontSystem,
        metrics: Metrics,
        cell_width: f32,
        palette: Palette,
    ) -> usize {
        for index in 0..self.cells.len() {
            self.rebuild_row(index, font_system, metrics, cell_width, palette);
        }
        self.cells.len()
    }

    fn rebuild_row(
        &mut self,
        index: usize,
        font_system: &mut FontSystem,
        metrics: Metrics,
        cell_width: f32,
        palette: Palette,
    ) -> Shaping {
        if index >= self.row_plans.len() || index >= self.cells.len() {
            return Shaping::Advanced;
        }
        self.next_generation = self.next_generation.saturating_add(1);
        let generation = self.next_generation;
        let font_family = self.font_family.as_str();
        let row = &mut self.row_plans[index];
        row.generation = generation;
        row.text.clear();
        row.styles.clear();
        row.backgrounds.clear();
        row.blocks.clear();
        row.boxes.clear();
        row.braille.clear();
        row.decorations.clear();
        let mut wide_glyph_count = 0;
        let mut ascii_fast_path = true;
        let mut background_start = 0;
        let mut current_background = None;
        for (column, cell) in self.cells[index].iter().enumerate() {
            let (foreground, background) = palette.resolved_colors(cell);
            if current_background != Some(background) {
                if let Some(color) = current_background
                    && color != palette.background()
                {
                    row.backgrounds.push(BackgroundRun {
                        start_column: background_start,
                        end_column: column,
                        color,
                    });
                }
                background_start = column;
                current_background = Some(background);
            }
            let style = TextStyle {
                foreground,
                bold: cell.flags.contains(CellFlags::BOLD),
                italic: cell.flags.contains(CellFlags::ITALIC),
            };
            ascii_fast_path &= !style.bold
                && !style.italic
                && (cell.text.is_empty()
                    || (cell.text.is_ascii() && cell.text.chars().count() <= 1)
                    || geometric_block_shape(cell.text.as_str()).is_some()
                    || geometric_box_shape(cell.text.as_str()).is_some()
                    || geometric_braille_pattern(cell.text.as_str()).is_some());
            let block_shape = if cell
                .flags
                .intersects(CellFlags::WIDE | CellFlags::WIDE_SPACER)
            {
                None
            } else {
                geometric_block_shape(cell.text.as_str())
            };
            let box_shape = if cell
                .flags
                .intersects(CellFlags::WIDE | CellFlags::WIDE_SPACER)
            {
                None
            } else {
                geometric_box_shape(cell.text.as_str())
            };
            let braille_pattern = if cell
                .flags
                .intersects(CellFlags::WIDE | CellFlags::WIDE_SPACER)
            {
                None
            } else {
                geometric_braille_pattern(cell.text.as_str())
            };
            if let Some(shape) = block_shape {
                row.blocks.push(BlockPrimitive {
                    column,
                    shape,
                    foreground,
                });
            }
            if let Some(shape) = box_shape {
                row.boxes.push(BoxPrimitive {
                    column,
                    shape,
                    foreground,
                });
            }
            if let Some(pattern) = braille_pattern {
                row.braille.push(BraillePrimitive {
                    column,
                    pattern,
                    foreground,
                });
            }
            if cell
                .flags
                .intersects(CellFlags::UNDERLINE | CellFlags::DOUBLE_UNDERLINE)
            {
                row.decorations.push(DecorationPrimitive {
                    column,
                    kind: DecorationKind::Underline,
                    foreground,
                });
            }
            if cell.flags.contains(CellFlags::STRIKEOUT) {
                row.decorations.push(DecorationPrimitive {
                    column,
                    kind: DecorationKind::Strikeout,
                    foreground,
                });
            }
            if cell.flags.contains(CellFlags::WIDE) && !cell.text.is_empty() {
                if wide_glyph_count != 0
                    && row.wide_glyphs[wide_glyph_count - 1].can_append(column, style)
                {
                    row.wide_glyphs[wide_glyph_count - 1].append(cell.text.as_str());
                } else {
                    if wide_glyph_count == row.wide_glyphs.len() {
                        row.wide_glyphs.push(WideGlyph::new(font_system, metrics));
                    }
                    row.wide_glyphs[wide_glyph_count].begin(column, cell.text.as_str(), style);
                    wide_glyph_count += 1;
                }
                ascii_fast_path = false;
            }
            let start = row.text.len();
            if block_shape.is_some()
                || box_shape.is_some()
                || braille_pattern.is_some()
                || cell
                    .flags
                    .intersects(CellFlags::WIDE | CellFlags::WIDE_SPACER)
            {
                // Preserve one shaped cell per grid column. Wide glyphs and
                // geometric blocks render separately so font metrics cannot
                // shift the cells that follow them.
                row.text.push(' ');
            } else if cell.text.is_empty() {
                row.text.push(' ');
            } else {
                row.text.push_str(cell.text.as_str());
            }
            let end = row.text.len();
            if let Some(run) = row.styles.last_mut()
                && run.style == style
            {
                run.end = end;
            } else {
                row.styles.push(TextRun { start, end, style });
            }
        }
        if let Some(color) = current_background
            && color != palette.background()
        {
            row.backgrounds.push(BackgroundRun {
                start_column: background_start,
                end_column: self.columns,
                color,
            });
        }
        row.wide_glyphs.truncate(wide_glyph_count);
        for wide in &mut row.wide_glyphs {
            wide.shape(font_system, metrics, cell_width, font_family);
        }
        let spans = row.styles.iter().map(|run| {
            (
                &row.text[run.start..run.end],
                attrs_for_style(run.style, font_family),
            )
        });
        let default_attrs = Attrs::new().family(Family::Name(font_family));
        row.buffer.set_size(
            Some(self.columns as f32 * cell_width),
            Some(metrics.line_height),
        );
        row.shaping = if ascii_fast_path {
            Shaping::Basic
        } else {
            Shaping::Advanced
        };
        row.buffer
            .set_rich_text(spans, &default_attrs, row.shaping, None);
        row.buffer.shape_until_scroll(font_system, false);
        row.shaping
    }
}

pub struct MetalRenderer {
    instance: Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: SurfaceConfiguration,
    font_system: FontSystem,
    swash_cache: SwashCache,
    glyph_cache: Cache,
    atlas: TextAtlas,
    text_rows: TextRows,
    search_buffer: Buffer,
    search_renderer: TextRenderer,
    search_viewport: Viewport,
    search_rect_renderer: RectRenderer,
    search_input: Option<SearchInput>,
    prepared_search_input: Option<SearchInput>,
    search_rectangle_scratch: Vec<RectInstance>,
    atlas_preparations_since_trim: u64,
    rect_renderer: RectRenderer,
    rounded_corner_renderer: RoundedCornerRenderer,
    braille_renderer: BrailleRenderer,
    geometry_transform_layout: wgpu::BindGroupLayout,
    geometry_transforms: RowTransforms,
    retained: RetainedGrid,
    preparation: RenderPreparationState,
    geometry_state: GeometryState,
    text_rows_dirty: Vec<bool>,
    geometry_row_generations: Vec<u64>,
    text_row_generations: Vec<u64>,
    rectangle_scratch: Vec<RectInstance>,
    rounded_corner_scratch: Vec<RoundedCornerInstance>,
    braille_scratch: Vec<BrailleInstance>,
    metrics: RendererMetrics,
    measurement: MeasurementState,
    cursor: CursorState,
    selection: Option<SelectionRange>,
    palette: Palette,
    cursor_blink_visible: bool,
    occluded: bool,
    pending_frame: bool,
    font_size: f32,
    padding: f32,
    scale_factor: f64,
    cell_metrics: CellMetrics,
    adapter_name: String,
    window: Arc<Window>,
}

impl std::fmt::Debug for MetalRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalRenderer")
            .field("columns", &self.retained.columns)
            .field("rows", &self.retained.rows)
            .field("font_size", &self.font_size)
            .field("cell_metrics", &self.cell_metrics)
            .field("adapter_name", &self.adapter_name)
            .finish_non_exhaustive()
    }
}

impl MetalRenderer {
    /// Creates a renderer backed exclusively by Apple's Metal API.
    ///
    /// # Errors
    ///
    /// Returns an error if a Metal adapter/device/surface cannot be created or configured.
    pub async fn new(
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
        config: RendererConfig,
    ) -> Result<Self> {
        let physical_size = nonzero_size(window.inner_size());
        let scale_factor = window.scale_factor();
        let mut instance_descriptor = InstanceDescriptor::new_with_display_handle(Box::new(
            event_loop.owned_display_handle(),
        ));
        instance_descriptor.backends = Backends::METAL;
        let instance = Instance::new(instance_descriptor);
        let surface = instance
            .create_surface(Arc::clone(&window))
            .context("creating Metal presentation surface")?;
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                ..RequestAdapterOptions::default()
            })
            .await
            .context("requesting Metal GPU adapter")?;
        let adapter_info = adapter.get_info();
        if adapter_info.backend != wgpu::Backend::Metal {
            bail!("Metal backend required, got {:?}", adapter_info.backend);
        }
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("Tmon device"),
                ..DeviceDescriptor::default()
            })
            .await
            .context("requesting Metal device")?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(TextureFormat::is_srgb)
            .unwrap_or(TextureFormat::Bgra8UnormSrgb);
        let present_mode = if capabilities.present_modes.contains(&PresentMode::AutoVsync) {
            PresentMode::AutoVsync
        } else {
            PresentMode::Fifo
        };
        let surface_config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: physical_size.width,
            height: physical_size.height,
            present_mode,
            desired_maximum_frame_latency: 1,
            alpha_mode: CompositeAlphaMode::Opaque,
            view_formats: Vec::new(),
            color_space: SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &surface_config);

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(&config.font_family);
        let physical_font_size = config.font_size * scale_factor as f32;
        let cell_metrics = measure_cell(&mut font_system, physical_font_size);
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_rows = TextRows::new();
        let search_buffer = Buffer::new(
            &mut font_system,
            Metrics::new(
                SEARCH_FONT_SIZE * scale_factor as f32,
                SEARCH_LINE_HEIGHT * scale_factor as f32,
            ),
        );
        let search_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let search_viewport = Viewport::new(&device, &cache);
        let geometry_transform_layout = RowTransforms::layout(&device);
        let geometry_transforms = RowTransforms::new(&device, &geometry_transform_layout, 0);
        let rect_renderer = RectRenderer::new(&device, format, &geometry_transform_layout);
        let search_rect_renderer = RectRenderer::new(&device, format, &geometry_transform_layout);
        let rounded_corner_renderer =
            RoundedCornerRenderer::new(&device, format, &geometry_transform_layout);
        let braille_renderer = BrailleRenderer::new(&device, format, &geometry_transform_layout);

        Ok(Self {
            instance,
            device,
            queue,
            surface,
            surface_config,
            font_system,
            swash_cache,
            glyph_cache: cache,
            atlas,
            text_rows,
            search_buffer,
            search_renderer,
            search_viewport,
            search_rect_renderer,
            search_input: None,
            prepared_search_input: None,
            search_rectangle_scratch: Vec::new(),
            atlas_preparations_since_trim: 0,
            rect_renderer,
            rounded_corner_renderer,
            braille_renderer,
            geometry_transform_layout,
            geometry_transforms,
            retained: RetainedGrid::with_font_family(config.font_family),
            preparation: RenderPreparationState::default(),
            geometry_state: GeometryState {
                dynamic: true,
                transforms: true,
                ..GeometryState::default()
            },
            text_rows_dirty: Vec::new(),
            geometry_row_generations: Vec::new(),
            text_row_generations: Vec::new(),
            rectangle_scratch: Vec::new(),
            rounded_corner_scratch: Vec::new(),
            braille_scratch: Vec::new(),
            metrics: RendererMetrics::default(),
            measurement: MeasurementState::default(),
            cursor: CursorState::default(),
            selection: None,
            palette: Palette::new(config.theme),
            cursor_blink_visible: true,
            occluded: false,
            pending_frame: false,
            font_size: config.font_size,
            padding: config.padding,
            scale_factor,
            cell_metrics,
            adapter_name: adapter_info.name,
            window,
        })
    }

    /// Moves presentation to another native window while retaining the Metal device, glyph atlas,
    /// font database, pipelines, and reusable frame storage.
    ///
    /// Native macOS tabs are separate `NSWindow` instances. Retargeting the surface lets each tab
    /// own a real window without multiplying the renderer's expensive GPU and shaping resources.
    ///
    /// # Errors
    ///
    /// Returns an error when a Metal surface cannot be created for the new window.
    pub fn retarget_window(&mut self, window: Arc<Window>) -> Result<()> {
        let surface = self
            .instance
            .create_surface(Arc::clone(&window))
            .context("creating Metal surface for native tab")?;
        let physical_size = nonzero_size(window.inner_size());
        self.surface_config.width = physical_size.width;
        self.surface_config.height = physical_size.height;
        surface.configure(&self.device, &self.surface_config);

        let scale_factor = window.scale_factor();
        self.surface = surface;
        self.window = window;
        self.occluded = false;
        self.prepared_search_input = None;
        if (self.scale_factor - scale_factor).abs() < f64::EPSILON {
            self.geometry_state.transforms = true;
            self.mark_frame_pending();
            self.window.request_redraw();
        } else {
            self.scale_factor = scale_factor;
            self.remeasure_and_rebuild();
        }
        Ok(())
    }

    #[must_use]
    pub const fn cell_metrics(&self) -> CellMetrics {
        self.cell_metrics
    }

    #[must_use]
    pub fn grid_dimensions(&self) -> (usize, usize) {
        self.grid_dimensions_for_size(PhysicalSize::new(
            self.surface_config.width,
            self.surface_config.height,
        ))
    }

    #[must_use]
    pub fn grid_dimensions_for_size(&self, size: PhysicalSize<u32>) -> (usize, usize) {
        let padding = self.physical_padding() * 2.0;
        let width = (size.width as f32 - padding).max(self.cell_metrics.width * 2.0);
        let height = (size.height as f32 - padding).max(self.cell_metrics.height);
        (
            (width / self.cell_metrics.width).floor() as usize,
            (height / self.cell_metrics.height).floor() as usize,
        )
    }

    #[must_use]
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    #[must_use]
    pub fn window(&self) -> &Window {
        &self.window
    }

    #[must_use]
    pub const fn surface_size(&self) -> PhysicalSize<u32> {
        PhysicalSize::new(self.surface_config.width, self.surface_config.height)
    }

    #[must_use]
    pub const fn metrics(&self) -> RendererMetrics {
        self.metrics
    }

    pub fn reset_metrics(&mut self) {
        self.metrics = RendererMetrics::default();
    }

    /// Enables or disables per-frame clock sampling.
    ///
    /// This is intended for the native release benchmark. It is off by default so ordinary
    /// rendering does not read the clock or retain frame samples.
    pub fn set_measurement_enabled(&mut self, enabled: bool) {
        self.measurement.enabled = enabled;
        self.measurement.pending_apply_ns = 0;
        self.measurement.last_frame = None;
    }

    /// Sets the deadline used by the opt-in missed-refresh counter.
    pub fn set_frame_budget_ns(&mut self, frame_budget_ns: Option<u64>) {
        self.measurement.frame_budget_ns = frame_budget_ns;
    }

    /// Takes the latest measured presented frame, if measurement is enabled.
    pub fn take_last_frame_timings(&mut self) -> Option<RendererFrameTimings> {
        self.measurement.last_frame.take()
    }

    #[must_use]
    pub fn cell_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        let padding = f64::from(self.physical_padding());
        if x < padding || y < padding {
            return None;
        }
        let column = ((x - padding) / f64::from(self.cell_metrics.width)).floor() as usize;
        let row = ((y - padding) / f64::from(self.cell_metrics.height)).floor() as usize;
        (column < self.retained.columns && row < self.retained.rows).then_some((column, row))
    }

    #[must_use]
    pub fn closest_cell_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        if self.retained.columns == 0 || self.retained.rows == 0 {
            return None;
        }
        let padding = f64::from(self.physical_padding());
        let dimensions = self.terminal_pixel_dimensions();
        let pixel_x = (x - padding)
            .floor()
            .clamp(0.0, f64::from(dimensions.0.saturating_sub(1)));
        let pixel_y = (y - padding)
            .floor()
            .clamp(0.0, f64::from(dimensions.1.saturating_sub(1)));
        Some((
            ((pixel_x / f64::from(self.cell_metrics.width)).floor() as usize)
                .min(self.retained.columns - 1),
            ((pixel_y / f64::from(self.cell_metrics.height)).floor() as usize)
                .min(self.retained.rows - 1),
        ))
    }

    #[must_use]
    pub fn terminal_pixel_dimensions(&self) -> (u32, u32) {
        text_view_dimensions(
            self.retained.columns,
            self.retained.rows,
            self.cell_metrics.width,
            self.cell_metrics.height,
        )
    }

    #[must_use]
    pub fn terminal_pixel_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        text_view_pixel_at(
            x,
            y,
            f64::from(self.physical_padding()),
            self.terminal_pixel_dimensions(),
        )
    }

    #[must_use]
    pub fn closest_terminal_pixel_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        let (width, height) = self.terminal_pixel_dimensions();
        if width == 0 || height == 0 {
            return None;
        }
        let padding = f64::from(self.physical_padding());
        Some((
            (x - padding).floor().clamp(0.0, f64::from(width - 1)) as usize,
            (y - padding).floor().clamp(0.0, f64::from(height - 1)) as usize,
        ))
    }

    pub fn resize_surface(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            self.occluded = true;
            return;
        }
        if self.surface_config.width == size.width && self.surface_config.height == size.height {
            let was_occluded = std::mem::replace(&mut self.occluded, false);
            if was_occluded {
                self.window.request_redraw();
            }
            return;
        }
        self.occluded = false;
        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.surface.configure(&self.device, &self.surface_config);
        self.geometry_state.transforms = true;
        self.prepared_search_input = None;
        self.mark_frame_pending();
        self.window.request_redraw();
    }

    pub fn set_occluded(&mut self, occluded: bool) {
        let request_redraw = redraw_after_occlusion_change(self.occluded, occluded);
        self.occluded = occluded;
        if request_redraw {
            self.window.request_redraw();
        }
    }

    #[must_use]
    pub const fn is_occluded(&self) -> bool {
        self.occluded
    }

    #[must_use]
    pub const fn cursor_should_blink(&self) -> bool {
        cursor_should_blink(self.cursor, self.occluded)
    }

    pub fn toggle_cursor_blink(&mut self) {
        if self.cursor_should_blink() {
            self.cursor_blink_visible = !self.cursor_blink_visible;
            self.geometry_state.dynamic = true;
            self.mark_frame_pending();
            self.window.request_redraw();
        }
    }

    pub fn set_scale_factor(&mut self, scale_factor: f64) {
        if (self.scale_factor - scale_factor).abs() < f64::EPSILON {
            return;
        }
        self.scale_factor = scale_factor;
        self.prepared_search_input = None;
        self.remeasure_and_rebuild();
    }

    pub fn set_font_size(&mut self, font_size: f32) {
        let font_size = font_size.clamp(8.0, 36.0);
        if (self.font_size - font_size).abs() < f32::EPSILON {
            return;
        }
        self.font_size = font_size;
        self.remeasure_and_rebuild();
    }

    #[must_use]
    pub const fn font_size(&self) -> f32 {
        self.font_size
    }

    pub fn set_search_input(&mut self, input: Option<SearchInput>) {
        if self.search_input == input {
            return;
        }
        self.search_input = input;
        self.geometry_state.dynamic = true;
        self.mark_frame_pending();
        self.window.request_redraw();
    }

    pub fn set_dynamic_color(&mut self, target: DynamicColor, color: [u8; 3]) {
        if self.palette.set(target, color) {
            self.dynamic_color_changed(target);
        }
    }

    pub fn reset_dynamic_color(&mut self, target: DynamicColor) {
        if self.palette.reset(target) {
            self.dynamic_color_changed(target);
        }
    }

    fn dynamic_color_changed(&mut self, target: DynamicColor) {
        if matches!(target, DynamicColor::Foreground | DynamicColor::Background) {
            let metrics = self.text_metrics();
            let rebuilt_rows = self.retained.rebuild_all(
                &mut self.font_system,
                metrics,
                self.cell_metrics.width,
                self.palette,
            );
            self.record_rebuilt_rows(rebuilt_rows);
            self.mark_all_text_rows_dirty();
            self.mark_all_static_geometry_dirty();
        }
        if target == DynamicColor::Cursor {
            self.geometry_state.dynamic = true;
        }
        self.mark_frame_pending();
        self.window.request_redraw();
    }

    fn remeasure_and_rebuild(&mut self) {
        self.cell_metrics = measure_cell(
            &mut self.font_system,
            self.font_size * self.scale_factor as f32,
        );
        let metrics = self.text_metrics();
        let rebuilt_rows = self.retained.remeasure_and_rebuild(
            &mut self.font_system,
            metrics,
            self.cell_metrics.width,
            self.palette,
        );
        self.record_rebuilt_rows(rebuilt_rows);
        self.mark_all_text_rows_dirty();
        self.preparation.mark_viewport_dirty();
        self.mark_all_static_geometry_dirty();
        self.geometry_state.transforms = true;
        self.geometry_state.dynamic = true;
        self.mark_frame_pending();
        self.window.request_redraw();
    }

    pub fn apply_frame(&mut self, update: &FrameUpdate) {
        let measurement_started = self.measurement.enabled.then(Instant::now);
        self.metrics.frame_updates = self.metrics.frame_updates.saturating_add(1);
        self.mark_frame_pending();
        let previous_cursor = self.cursor;
        let previous_selection = self.selection;
        let previous_blink_visible = self.cursor_blink_visible;
        if update.has_damage() {
            self.cursor_blink_visible = true;
        }
        self.cursor = update.cursor;
        self.selection = update.selection;
        let metrics = self.text_metrics();
        let retained_update = self.retained.apply_frame(
            update,
            &mut self.font_system,
            metrics,
            self.cell_metrics.width,
            self.palette,
        );
        if retained_update.dimensions_changed {
            self.resize_geometry_rows(update.rows);
            self.mark_all_static_geometry_dirty();
            self.mark_all_text_rows_dirty();
            self.geometry_state.transforms = true;
        } else {
            for movement in &update.row_moves {
                self.move_static_geometry_rows(*movement);
            }
        }
        if update.full {
            self.mark_all_static_geometry_dirty();
            self.mark_all_text_rows_dirty();
        } else {
            for row in &update.row_updates {
                let generation = self
                    .retained
                    .row_plans
                    .get(row.index)
                    .map_or(0, |plan| plan.generation);
                if self
                    .geometry_row_generations
                    .get(row.index)
                    .is_some_and(|cached| *cached != generation)
                {
                    self.geometry_state.static_rows[row.index] = true;
                }
                if self
                    .text_row_generations
                    .get(row.index)
                    .is_some_and(|cached| *cached != generation)
                {
                    self.text_rows_dirty[row.index] = true;
                }
            }
        }
        if previous_cursor != self.cursor
            || previous_selection != self.selection
            || previous_blink_visible != self.cursor_blink_visible
            || update.full
            || retained_update.rebuilt_rows != 0
        {
            self.geometry_state.dynamic = true;
        }
        self.record_rebuilt_rows(retained_update.rebuilt_rows);
        self.metrics.ascii_rows_shaped = self
            .metrics
            .ascii_rows_shaped
            .saturating_add(u64::try_from(retained_update.ascii_rows_shaped).unwrap_or(u64::MAX));
        self.metrics.complex_rows_shaped = self
            .metrics
            .complex_rows_shaped
            .saturating_add(u64::try_from(retained_update.complex_rows_shaped).unwrap_or(u64::MAX));
        if retained_update.text_changed() {
            self.preparation.mark_text_dirty();
        }
        if update.has_damage() {
            self.window.request_redraw();
        }
        if let Some(started) = measurement_started {
            self.measurement.pending_apply_ns = duration_ns(started.elapsed());
        }
    }

    fn record_rebuilt_rows(&mut self, rebuilt_rows: usize) {
        let rebuilt_rows = u64::try_from(rebuilt_rows).unwrap_or(u64::MAX);
        self.metrics.rebuilt_rows = self.metrics.rebuilt_rows.saturating_add(rebuilt_rows);
        self.metrics.row_rectangle_plans_rebuilt = self
            .metrics
            .row_rectangle_plans_rebuilt
            .saturating_add(rebuilt_rows);
    }

    fn resize_geometry_rows(&mut self, rows: usize) {
        self.rect_renderer.resize_rows(&self.device, rows);
        self.rounded_corner_renderer.resize_rows(&self.device, rows);
        self.braille_renderer.resize_rows(&self.device, rows);
        self.geometry_transforms
            .resize(&self.device, &self.geometry_transform_layout, rows);
        self.geometry_state.static_rows.resize(rows, true);
        self.text_rows
            .resize(&self.device, &self.glyph_cache, &mut self.atlas, rows);
        self.text_rows_dirty.resize(rows, true);
        self.geometry_row_generations.resize(rows, 0);
        self.text_row_generations.resize(rows, 0);
        self.preparation.mark_viewport_dirty();
    }

    fn move_static_geometry_rows(&mut self, movement: RowMove) {
        if !self.rect_renderer.move_rows(movement)
            || !self.rounded_corner_renderer.move_rows(movement)
            || !self.braille_renderer.move_rows(movement)
        {
            self.mark_all_static_geometry_dirty();
            self.mark_all_text_rows_dirty();
            return;
        }
        let height = movement.end_row.saturating_sub(movement.start_row);
        if movement.start_row >= movement.end_row
            || movement.end_row > self.geometry_state.static_rows.len()
            || movement.count == 0
            || movement.count >= height
        {
            self.mark_all_static_geometry_dirty();
            self.mark_all_text_rows_dirty();
            return;
        }
        let dirty = &mut self.geometry_state.static_rows[movement.start_row..movement.end_row];
        match movement.direction {
            RowMoveDirection::Up => dirty.rotate_left(movement.count),
            RowMoveDirection::Down => dirty.rotate_right(movement.count),
        }
        let generations = &mut self.geometry_row_generations[movement.start_row..movement.end_row];
        match movement.direction {
            RowMoveDirection::Up => generations.rotate_left(movement.count),
            RowMoveDirection::Down => generations.rotate_right(movement.count),
        }
        if !self.text_rows.move_rows(movement) {
            self.mark_all_text_rows_dirty();
            return;
        }
        let dirty = &mut self.text_rows_dirty[movement.start_row..movement.end_row];
        match movement.direction {
            RowMoveDirection::Up => dirty.rotate_left(movement.count),
            RowMoveDirection::Down => dirty.rotate_right(movement.count),
        }
        let generations = &mut self.text_row_generations[movement.start_row..movement.end_row];
        match movement.direction {
            RowMoveDirection::Up => generations.rotate_left(movement.count),
            RowMoveDirection::Down => generations.rotate_right(movement.count),
        }
    }

    fn mark_all_static_geometry_dirty(&mut self) {
        self.geometry_state.static_rows.fill(true);
    }

    fn mark_all_text_rows_dirty(&mut self) {
        self.text_rows_dirty.fill(true);
        self.preparation.mark_text_dirty();
    }

    /// Prepares damaged text plans, draws a frame, and presents it to the window surface.
    ///
    /// # Errors
    ///
    /// Returns an error if glyph preparation, surface recreation, or GPU rendering fails.
    pub fn render(&mut self) -> Result<RenderStatus> {
        let render_started = self.measurement.enabled.then(Instant::now);
        let mut frame_timings = RendererFrameTimings {
            apply_frame_ns: self.measurement.pending_apply_ns,
            ..RendererFrameTimings::default()
        };
        self.measurement.pending_apply_ns = 0;
        if self.occluded || self.surface_config.width == 0 || self.surface_config.height == 0 {
            self.metrics.occluded_frames = self.metrics.occluded_frames.saturating_add(1);
            self.metrics.skipped_frames = self.metrics.skipped_frames.saturating_add(1);
            return Ok(RenderStatus::Occluded);
        }

        let surface_acquire_started = self.measurement.enabled.then(Instant::now);
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.metrics.surface_retries = self.metrics.surface_retries.saturating_add(1);
                return Ok(RenderStatus::Retry);
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.metrics.occluded_frames = self.metrics.occluded_frames.saturating_add(1);
                self.metrics.skipped_frames = self.metrics.skipped_frames.saturating_add(1);
                return Ok(RenderStatus::Occluded);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.surface.configure(&self.device, &self.surface_config);
                self.preparation.mark_viewport_dirty();
                self.metrics.surface_retries = self.metrics.surface_retries.saturating_add(1);
                return Ok(RenderStatus::Retry);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(Arc::clone(&self.window))
                    .context("recreating lost Metal surface")?;
                self.surface.configure(&self.device, &self.surface_config);
                self.preparation.mark_viewport_dirty();
                self.metrics.surface_retries = self.metrics.surface_retries.saturating_add(1);
                return Ok(RenderStatus::Retry);
            }
            wgpu::CurrentSurfaceTexture::Validation => bail!("Metal surface validation error"),
        };
        self.metrics.acquired_frames = self.metrics.acquired_frames.saturating_add(1);
        if let Some(started) = surface_acquire_started {
            frame_timings.surface_acquire_ns = duration_ns(started.elapsed());
        }

        if self.preparation.viewport_is_dirty() {
            let viewport_started = self.measurement.enabled.then(Instant::now);
            let (width, height) = self.terminal_pixel_dimensions();
            for row in self.text_rows.iter_mut() {
                row.viewport.update(
                    &self.queue,
                    Resolution {
                        width: width.max(1),
                        height: height
                            .min(self.cell_metrics.height.round().max(1.0) as u32)
                            .max(1),
                    },
                );
            }
            self.preparation.viewport_updated();
            self.metrics.viewport_updates = self.metrics.viewport_updates.saturating_add(1);
            if let Some(started) = viewport_started {
                frame_timings.viewport_update_ns = duration_ns(started.elapsed());
            }
        }

        let has_dirty_text =
            self.preparation.text_is_dirty() && self.text_rows_dirty.iter().any(|dirty| *dirty);
        let trim_after_prepare = has_dirty_text
            && self.atlas_preparations_since_trim.saturating_add(1) >= ATLAS_TRIM_INTERVAL;
        if trim_after_prepare {
            // `glyphon` tracks atlas liveness from prepared glyphs. Refresh every retained row in
            // the trim generation so cached row vertices never reference an evicted glyph.
            self.text_rows_dirty.fill(true);
        }
        let prepared_text = has_dirty_text;
        if prepared_text {
            let glyph_prepare_started = self.measurement.enabled.then(Instant::now);
            let (terminal_width, _) = self.terminal_pixel_dimensions();
            let row_height = self.cell_metrics.height.round().max(1.0) as u32;
            let bounds = TextBounds {
                left: 0,
                top: 0,
                right: i32::try_from(terminal_width).unwrap_or(i32::MAX),
                bottom: i32::try_from(row_height).unwrap_or(i32::MAX),
            };
            let cell_width = self.cell_metrics.width;
            let default_foreground = self.palette.foreground();
            let default_color = glyphon::Color::rgb(
                default_foreground[0],
                default_foreground[1],
                default_foreground[2],
            );
            let dirty_count = self.text_rows_dirty.iter().filter(|dirty| **dirty).count();
            self.metrics.text_rows_reused = self.metrics.text_rows_reused.saturating_add(
                u64::try_from(self.retained.rows.saturating_sub(dirty_count)).unwrap_or(u64::MAX),
            );
            for row_index in 0..self.text_rows_dirty.len() {
                if !self.text_rows_dirty[row_index] {
                    continue;
                }
                let Some(plan) = self.retained.row_plans.get(row_index) else {
                    continue;
                };
                let Some(row_renderer) = self.text_rows.get_mut(row_index) else {
                    continue;
                };
                let row_area = TextArea {
                    buffer: &plan.buffer,
                    left: 0.0,
                    top: 0.0,
                    scale: 1.0,
                    bounds,
                    default_color,
                    custom_glyphs: &[],
                };
                let areas = std::iter::once(row_area).chain(plan.wide_glyphs.iter().map(|wide| {
                    let left = wide.column as f32 * cell_width;
                    TextArea {
                        buffer: &wide.buffer,
                        left,
                        top: 0.0,
                        scale: 1.0,
                        bounds: TextBounds {
                            left: bounds.left.max(left.floor() as i32),
                            top: bounds.top,
                            right: bounds
                                .right
                                .min((left + cell_width * wide.columns as f32).ceil() as i32),
                            bottom: bounds.bottom,
                        },
                        default_color,
                        custom_glyphs: &[],
                    }
                }));
                row_renderer
                    .renderer
                    .prepare(
                        &self.device,
                        &self.queue,
                        &mut self.font_system,
                        &mut self.atlas,
                        &row_renderer.viewport,
                        areas,
                        &mut self.swash_cache,
                    )
                    .context("preparing retained terminal row glyphs")?;
                self.text_rows_dirty[row_index] = false;
                self.text_row_generations[row_index] = plan.generation;
                self.metrics.text_row_prepares = self.metrics.text_row_prepares.saturating_add(1);
            }
            self.preparation.text_prepared();
            self.metrics.text_prepares = self.metrics.text_prepares.saturating_add(1);
            if let Some(started) = glyph_prepare_started {
                frame_timings.glyph_prepare_ns = duration_ns(started.elapsed());
            }
        }

        self.prepare_search_input(trim_after_prepare)?;

        let (geometry_build_ns, geometry_upload_ns) = self.update_geometry();
        frame_timings.geometry_build_ns = geometry_build_ns;
        frame_timings.geometry_upload_ns = geometry_upload_ns;
        let encoding_started = self.measurement.enabled.then(Instant::now);
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Tmon frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Tmon frame"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(wgpu_color(self.palette.background())),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.rect_renderer
                .draw(&mut pass, &self.geometry_transforms);
            self.rounded_corner_renderer
                .draw(&mut pass, &self.geometry_transforms);
            self.braille_renderer
                .draw(&mut pass, &self.geometry_transforms);
            let padding = self.physical_padding();
            let (terminal_width, _) = self.terminal_pixel_dimensions();
            for (row_index, row) in self.text_rows.iter().enumerate() {
                pass.set_viewport(
                    padding,
                    padding + row_index as f32 * self.cell_metrics.height,
                    terminal_width.max(1) as f32,
                    self.cell_metrics.height.max(1.0),
                    0.0,
                    1.0,
                );
                row.renderer
                    .render(&self.atlas, &row.viewport, &mut pass)
                    .context("rendering retained terminal row glyphs")?;
            }
            if self.search_input.is_some() {
                pass.set_viewport(
                    0.0,
                    0.0,
                    self.surface_config.width.max(1) as f32,
                    self.surface_config.height.max(1) as f32,
                    0.0,
                    1.0,
                );
                self.search_rect_renderer
                    .draw(&mut pass, &self.geometry_transforms);
                self.search_renderer
                    .render(&self.atlas, &self.search_viewport, &mut pass)
                    .context("rendering search input")?;
            }
        }
        if let Some(started) = encoding_started {
            frame_timings.encoding_ns = duration_ns(started.elapsed());
        }
        let submission_started = self.measurement.enabled.then(Instant::now);
        self.queue.submit(Some(encoder.finish()));
        if let Some(started) = submission_started {
            frame_timings.submission_ns = duration_ns(started.elapsed());
        }
        let presentation_started = self.measurement.enabled.then(Instant::now);
        self.queue.present(frame);
        if let Some(started) = presentation_started {
            frame_timings.presentation_ns = duration_ns(started.elapsed());
        }
        if prepared_text {
            if trim_after_prepare {
                self.atlas.trim();
                self.atlas_preparations_since_trim = 0;
                self.metrics.atlas_trims = self.metrics.atlas_trims.saturating_add(1);
            } else {
                self.atlas_preparations_since_trim =
                    self.atlas_preparations_since_trim.saturating_add(1);
            }
        }
        self.metrics.presented_frames = self.metrics.presented_frames.saturating_add(1);
        self.pending_frame = false;
        if let Some(started) = render_started {
            frame_timings.render_total_ns = duration_ns(started.elapsed());
            frame_timings.end_to_end_ns = frame_timings
                .apply_frame_ns
                .saturating_add(frame_timings.render_total_ns);
            if self
                .measurement
                .frame_budget_ns
                .is_some_and(|budget| frame_timings.end_to_end_ns > budget)
            {
                self.metrics.missed_refresh_deadlines =
                    self.metrics.missed_refresh_deadlines.saturating_add(1);
            }
            self.measurement.last_frame = Some(frame_timings);
        }
        Ok(RenderStatus::Presented)
    }

    fn prepare_search_input(&mut self, force: bool) -> Result<()> {
        if self.prepared_search_input == self.search_input && !force {
            return Ok(());
        }
        self.search_rectangle_scratch.clear();
        let Some(input) = &self.search_input else {
            self.search_rect_renderer.upload_overlay(
                &self.device,
                &self.queue,
                &self.search_rectangle_scratch,
            );
            self.prepared_search_input = None;
            return Ok(());
        };
        let layout = search_field_layout(
            self.surface_config.width as f32,
            self.surface_config.height as f32,
            self.scale_factor as f32,
        );
        let scale = self.scale_factor as f32;
        let display = search_field_text(&input.query, layout.text_width, scale);
        build_search_field_geometry(
            &mut self.search_rectangle_scratch,
            layout,
            self.palette,
            input.has_match,
            (!input.query.is_empty()).then(|| display.chars().count()),
            self.surface_config.width as f32,
            self.surface_config.height as f32,
        );
        self.search_rect_renderer.upload_overlay(
            &self.device,
            &self.queue,
            &self.search_rectangle_scratch,
        );

        self.search_buffer.set_metrics_and_size(
            Metrics::new(SEARCH_FONT_SIZE * scale, SEARCH_LINE_HEIGHT * scale),
            Some(layout.text_width),
            Some(SEARCH_LINE_HEIGHT * scale),
        );
        self.search_buffer.set_wrap(Wrap::None);
        self.search_buffer.set_text(
            &display,
            &Attrs::new()
                .family(Family::Name(&self.retained.font_family))
                .weight(Weight::NORMAL),
            Shaping::Advanced,
            None,
        );
        self.search_buffer
            .shape_until_scroll(&mut self.font_system, false);
        self.search_viewport.update(
            &self.queue,
            Resolution {
                width: self.surface_config.width.max(1),
                height: self.surface_config.height.max(1),
            },
        );
        let color = self.palette.search_foreground();
        self.search_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.search_viewport,
                [TextArea {
                    buffer: &self.search_buffer,
                    left: layout.text_x,
                    top: layout.text_y,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: layout.text_x.floor() as i32,
                        top: layout.y.floor() as i32,
                        right: (layout.text_x + layout.text_width).ceil() as i32,
                        bottom: (layout.y + layout.height).ceil() as i32,
                    },
                    default_color: glyphon::Color::rgb(color[0], color[1], color[2]),
                    custom_glyphs: &[],
                }],
                &mut self.swash_cache,
            )
            .context("preparing search input glyphs")?;
        self.prepared_search_input = self.search_input.clone();
        Ok(())
    }

    fn update_geometry(&mut self) -> (u64, u64) {
        let mut build_ns = 0_u64;
        let mut upload_ns = 0_u64;
        if self.geometry_state.transforms {
            let started = self.measurement.enabled.then(Instant::now);
            let bytes = self.geometry_transforms.update(
                &self.queue,
                self.surface_config.width as f32,
                self.surface_config.height as f32,
                self.physical_padding(),
                self.cell_metrics.height,
            );
            self.geometry_state.transforms = false;
            self.metrics.transform_writes = self.metrics.transform_writes.saturating_add(1);
            self.metrics.transform_bytes = self.metrics.transform_bytes.saturating_add(bytes);
            self.metrics.upload_bytes = self.metrics.upload_bytes.saturating_add(bytes);
            if let Some(started) = started {
                upload_ns = upload_ns.saturating_add(duration_ns(started.elapsed()));
            }
        }

        let dirty_rows = self
            .geometry_state
            .static_rows
            .iter()
            .filter(|dirty| **dirty)
            .count();
        self.metrics.static_rows_reused = self.metrics.static_rows_reused.saturating_add(
            u64::try_from(self.retained.rows.saturating_sub(dirty_rows)).unwrap_or(u64::MAX),
        );
        let frame = RectangleFrame {
            selection: None,
            cursor: CursorState::default(),
            cursor_blink_visible: false,
            padding: 0.0,
            cell_metrics: self.cell_metrics,
            font_size: self.font_size * self.scale_factor as f32,
            viewport_width: self.surface_config.width as f32,
            viewport_height: self.surface_config.height as f32,
            cursor_color: self.palette.cursor(),
            selection_color: self.palette.selection(),
        };
        for row_index in 0..self.geometry_state.static_rows.len() {
            if !self.geometry_state.static_rows[row_index] {
                continue;
            }
            let previous_rectangle_capacity = self.rectangle_scratch.capacity();
            let previous_corner_capacity = self.rounded_corner_scratch.capacity();
            let previous_braille_capacity = self.braille_scratch.capacity();
            let started = self.measurement.enabled.then(Instant::now);
            self.rectangle_scratch.clear();
            self.rounded_corner_scratch.clear();
            self.braille_scratch.clear();
            if let Some(plan) = self.retained.row_plans.get(row_index) {
                build_static_row_geometry(
                    &mut self.rectangle_scratch,
                    &mut self.rounded_corner_scratch,
                    &mut self.braille_scratch,
                    plan,
                    frame,
                );
            }
            if let Some(started) = started {
                build_ns = build_ns.saturating_add(duration_ns(started.elapsed()));
            }
            self.record_scratch_growth(
                previous_rectangle_capacity,
                previous_corner_capacity,
                previous_braille_capacity,
            );
            self.metrics.static_geometry_builds =
                self.metrics.static_geometry_builds.saturating_add(1);
            self.metrics.rectangle_builds = self.metrics.rectangle_builds.saturating_add(1);
            self.metrics.rounded_corner_builds =
                self.metrics.rounded_corner_builds.saturating_add(1);
            self.metrics.braille_builds = self.metrics.braille_builds.saturating_add(1);

            let rectangle_count = self.rectangle_scratch.len();
            let corner_count = self.rounded_corner_scratch.len();
            let braille_count = self.braille_scratch.len();
            let started = self.measurement.enabled.then(Instant::now);
            let rectangle = self.rect_renderer.upload_row(
                &self.device,
                &self.queue,
                row_index,
                &self.rectangle_scratch,
            );
            let corner = self.rounded_corner_renderer.upload_row(
                &self.device,
                &self.queue,
                row_index,
                &self.rounded_corner_scratch,
            );
            let braille = self.braille_renderer.upload_row(
                &self.device,
                &self.queue,
                row_index,
                &self.braille_scratch,
            );
            if let Some(started) = started {
                upload_ns = upload_ns.saturating_add(duration_ns(started.elapsed()));
            }
            self.record_static_uploads(
                rectangle,
                corner,
                braille,
                rectangle_count,
                corner_count,
                braille_count,
            );
            self.geometry_state.static_rows[row_index] = false;
            self.geometry_row_generations[row_index] = self
                .retained
                .row_plans
                .get(row_index)
                .map_or(0, |plan| plan.generation);
        }

        if self.geometry_state.dynamic {
            let padding = self.physical_padding();
            let started = self.measurement.enabled.then(Instant::now);
            build_dynamic_geometry_into(
                &mut self.rectangle_scratch,
                &self.retained,
                RectangleFrame {
                    selection: self.selection,
                    cursor: if self.search_input.is_some() {
                        CursorState {
                            visible: false,
                            ..self.cursor
                        }
                    } else {
                        self.cursor
                    },
                    cursor_blink_visible: self.cursor_blink_visible,
                    padding,
                    ..frame
                },
            );
            if let Some(started) = started {
                build_ns = build_ns.saturating_add(duration_ns(started.elapsed()));
            }
            self.metrics.dynamic_geometry_builds =
                self.metrics.dynamic_geometry_builds.saturating_add(1);
            let rectangle_count = self.rectangle_scratch.len();
            let started = self.measurement.enabled.then(Instant::now);
            let stats = self.rect_renderer.upload_overlay(
                &self.device,
                &self.queue,
                &self.rectangle_scratch,
            );
            if let Some(started) = started {
                upload_ns = upload_ns.saturating_add(duration_ns(started.elapsed()));
            }
            self.metrics.dynamic_geometry_writes = self
                .metrics
                .dynamic_geometry_writes
                .saturating_add(stats.writes);
            self.metrics.dynamic_geometry_bytes = self
                .metrics
                .dynamic_geometry_bytes
                .saturating_add(stats.bytes);
            self.metrics.geometry_buffer_growths = self
                .metrics
                .geometry_buffer_growths
                .saturating_add(stats.growths);
            self.metrics.rectangle_uploads =
                self.metrics.rectangle_uploads.saturating_add(stats.writes);
            self.metrics.rectangle_instances_uploaded = self
                .metrics
                .rectangle_instances_uploaded
                .saturating_add(u64::try_from(rectangle_count).unwrap_or(u64::MAX));
            self.metrics.upload_bytes = self.metrics.upload_bytes.saturating_add(stats.bytes);
            self.geometry_state.dynamic = false;
        }
        (build_ns, upload_ns)
    }

    fn mark_frame_pending(&mut self) {
        if self.pending_frame {
            self.metrics.coalesced_updates = self.metrics.coalesced_updates.saturating_add(1);
        }
        self.pending_frame = true;
    }

    fn record_scratch_growth(
        &mut self,
        rectangle_capacity: usize,
        corner_capacity: usize,
        braille_capacity: usize,
    ) {
        if self.rectangle_scratch.capacity() > rectangle_capacity {
            self.metrics.rectangle_scratch_growths =
                self.metrics.rectangle_scratch_growths.saturating_add(1);
        }
        if self.rounded_corner_scratch.capacity() > corner_capacity {
            self.metrics.rounded_corner_scratch_growths = self
                .metrics
                .rounded_corner_scratch_growths
                .saturating_add(1);
        }
        if self.braille_scratch.capacity() > braille_capacity {
            self.metrics.braille_scratch_growths =
                self.metrics.braille_scratch_growths.saturating_add(1);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_static_uploads(
        &mut self,
        rectangle: UploadStats,
        corner: UploadStats,
        braille: UploadStats,
        rectangle_count: usize,
        corner_count: usize,
        braille_count: usize,
    ) {
        let writes = rectangle
            .writes
            .saturating_add(corner.writes)
            .saturating_add(braille.writes);
        let bytes = rectangle
            .bytes
            .saturating_add(corner.bytes)
            .saturating_add(braille.bytes);
        let growths = rectangle
            .growths
            .saturating_add(corner.growths)
            .saturating_add(braille.growths);
        self.metrics.static_geometry_writes =
            self.metrics.static_geometry_writes.saturating_add(writes);
        self.metrics.static_geometry_bytes =
            self.metrics.static_geometry_bytes.saturating_add(bytes);
        self.metrics.geometry_buffer_growths =
            self.metrics.geometry_buffer_growths.saturating_add(growths);
        self.metrics.rectangle_uploads = self
            .metrics
            .rectangle_uploads
            .saturating_add(rectangle.writes);
        self.metrics.rounded_corner_uploads = self
            .metrics
            .rounded_corner_uploads
            .saturating_add(corner.writes);
        self.metrics.braille_uploads = self.metrics.braille_uploads.saturating_add(braille.writes);
        self.metrics.rectangle_instances_uploaded = self
            .metrics
            .rectangle_instances_uploaded
            .saturating_add(u64::try_from(rectangle_count).unwrap_or(u64::MAX));
        self.metrics.rounded_corner_instances_uploaded = self
            .metrics
            .rounded_corner_instances_uploaded
            .saturating_add(u64::try_from(corner_count).unwrap_or(u64::MAX));
        self.metrics.braille_instances_uploaded = self
            .metrics
            .braille_instances_uploaded
            .saturating_add(u64::try_from(braille_count).unwrap_or(u64::MAX));
        self.metrics.upload_bytes = self.metrics.upload_bytes.saturating_add(bytes);
    }

    fn physical_padding(&self) -> f32 {
        self.padding * self.scale_factor as f32
    }

    fn text_metrics(&self) -> Metrics {
        Metrics::new(
            self.font_size * self.scale_factor as f32,
            self.cell_metrics.height,
        )
    }
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn search_field_layout(viewport_width: f32, viewport_height: f32, scale: f32) -> SearchFieldLayout {
    let margin = (SEARCH_FIELD_MARGIN * scale).min(viewport_width / 4.0);
    let width = (SEARCH_FIELD_WIDTH * scale).min((viewport_width - margin * 2.0).max(1.0));
    let height = (SEARCH_FIELD_HEIGHT * scale).min((viewport_height - margin * 2.0).max(1.0));
    let inset = (SEARCH_FIELD_INSET * scale).min(width / 3.0);
    SearchFieldLayout {
        x: (viewport_width - margin - width).max(0.0),
        y: margin,
        width,
        height,
        text_x: (viewport_width - margin - width).max(0.0) + inset,
        text_y: margin + ((height - SEARCH_LINE_HEIGHT * scale) / 2.0).max(0.0),
        text_width: (width - inset * 2.0).max(1.0),
    }
}

fn search_field_text(query: &str, text_width: f32, scale: f32) -> String {
    if query.is_empty() {
        return "Search".to_owned();
    }
    let approximate_character_width = SEARCH_FONT_SIZE * 0.58 * scale;
    let max_characters = (text_width / approximate_character_width).floor().max(1.0) as usize;
    let character_count = query.chars().count();
    if character_count <= max_characters {
        return query.to_owned();
    }
    let tail_length = max_characters.saturating_sub(1);
    let mut tail = query.chars().rev().take(tail_length).collect::<Vec<_>>();
    tail.reverse();
    std::iter::once('…').chain(tail).collect()
}

fn build_search_field_geometry(
    rectangles: &mut Vec<RectInstance>,
    layout: SearchFieldLayout,
    palette: Palette,
    has_match: Option<bool>,
    caret_after_characters: Option<usize>,
    viewport_width: f32,
    viewport_height: f32,
) {
    let border = if has_match == Some(false) {
        palette.search_no_match()
    } else {
        palette.search_border()
    };
    let border_width = 1.0_f32.max(layout.height / SEARCH_FIELD_HEIGHT);
    rectangles.push(RectInstance::from_pixels(
        layout.x,
        layout.y + border_width * 2.0,
        layout.width,
        layout.height,
        [0, 0, 0],
        0.28,
        viewport_width,
        viewport_height,
    ));
    rectangles.push(RectInstance::from_pixels(
        layout.x,
        layout.y,
        layout.width,
        layout.height,
        palette.search_background(),
        0.97,
        viewport_width,
        viewport_height,
    ));
    for (x, y, width, height) in [
        (layout.x, layout.y, layout.width, border_width),
        (
            layout.x,
            layout.y + layout.height - border_width,
            layout.width,
            border_width,
        ),
        (layout.x, layout.y, border_width, layout.height),
        (
            layout.x + layout.width - border_width,
            layout.y,
            border_width,
            layout.height,
        ),
    ] {
        rectangles.push(RectInstance::from_pixels(
            x,
            y,
            width,
            height,
            border,
            1.0,
            viewport_width,
            viewport_height,
        ));
    }
    if let Some(character_count) = caret_after_characters {
        let character_width = SEARCH_FONT_SIZE * 0.6 * border_width;
        let caret_x = (layout.text_x + character_count as f32 * character_width)
            .min(layout.text_x + layout.text_width - border_width);
        rectangles.push(RectInstance::from_pixels(
            caret_x,
            layout.text_y + border_width * 2.0,
            border_width,
            (SEARCH_LINE_HEIGHT * border_width - border_width * 4.0).max(border_width),
            palette.search_foreground(),
            0.9,
            viewport_width,
            viewport_height,
        ));
    }
}

fn build_static_row_geometry(
    rectangles: &mut Vec<RectInstance>,
    rounded_corners: &mut Vec<RoundedCornerInstance>,
    braille_instances: &mut Vec<BrailleInstance>,
    plan: &RenderRow,
    frame: RectangleFrame,
) {
    let y = 0.0;
    for background in &plan.backgrounds {
        rectangles.push(RectInstance::from_pixels(
            background.start_column as f32 * frame.cell_metrics.width,
            y,
            (background.end_column - background.start_column) as f32 * frame.cell_metrics.width,
            frame.cell_metrics.height,
            background.color,
            1.0,
            frame.viewport_width,
            frame.viewport_height,
        ));
    }
    for block in &plan.blocks {
        let cell = rounded_cell_rect(
            0.0,
            frame.cell_metrics.width,
            frame.cell_metrics.height,
            block.column,
            0,
        );
        for rectangle in block_pixel_rectangles(block.shape, cell)
            .into_iter()
            .flatten()
        {
            if rectangle.right > rectangle.left && rectangle.bottom > rectangle.top {
                rectangles.push(RectInstance::from_pixels(
                    rectangle.left,
                    rectangle.top,
                    rectangle.right - rectangle.left,
                    rectangle.bottom - rectangle.top,
                    block.foreground,
                    1.0,
                    frame.viewport_width,
                    frame.viewport_height,
                ));
            }
        }
    }
    for box_primitive in &plan.boxes {
        let cell = rounded_cell_rect(
            0.0,
            frame.cell_metrics.width,
            frame.cell_metrics.height,
            box_primitive.column,
            0,
        );
        if let BoxDrawing::Rounded(corner) = box_primitive.shape {
            rounded_corners.push(RoundedCornerInstance::from_pixels(
                cell.left,
                cell.top,
                cell.right - cell.left,
                cell.bottom - cell.top,
                frame.font_size,
                corner,
                box_primitive.foreground,
                frame.viewport_width,
                frame.viewport_height,
            ));
            continue;
        }
        let metrics = BoxMetrics {
            width: cell.right - cell.left,
            height: cell.bottom - cell.top,
            font_size: frame.font_size,
        };
        for rectangle in box_rectangles(box_primitive.shape, metrics).as_slice() {
            rectangles.push(RectInstance::from_pixels(
                cell.left + rectangle.left,
                cell.top + rectangle.top,
                rectangle.right - rectangle.left,
                rectangle.bottom - rectangle.top,
                box_primitive.foreground,
                1.0,
                frame.viewport_width,
                frame.viewport_height,
            ));
        }
    }
    for braille in &plan.braille {
        if braille.pattern == 0 {
            continue;
        }
        let cell = rounded_cell_rect(
            0.0,
            frame.cell_metrics.width,
            frame.cell_metrics.height,
            braille.column,
            0,
        );
        braille_instances.push(BrailleInstance::from_pixels(
            cell.left,
            cell.top,
            cell.right - cell.left,
            cell.bottom - cell.top,
            braille.pattern,
            braille.foreground,
            frame.viewport_width,
            frame.viewport_height,
        ));
    }
    for decoration in &plan.decorations {
        let x = decoration.column as f32 * frame.cell_metrics.width;
        let (decoration_y, decoration_height) = match decoration.kind {
            DecorationKind::Underline => (frame.cell_metrics.height - 2.0, 1.0),
            DecorationKind::Strikeout => (frame.cell_metrics.height * 0.55, 1.0),
        };
        rectangles.push(RectInstance::from_pixels(
            x,
            decoration_y,
            frame.cell_metrics.width,
            decoration_height,
            decoration.foreground,
            1.0,
            frame.viewport_width,
            frame.viewport_height,
        ));
    }
}

fn build_dynamic_geometry_into(
    rectangles: &mut Vec<RectInstance>,
    retained: &RetainedGrid,
    frame: RectangleFrame,
) {
    rectangles.clear();
    if let Some(selection) = frame.selection {
        for row in selection.start.row..=selection.end.row.min(retained.rows.saturating_sub(1)) {
            if let Some((start, end)) = selection_span_for_row(selection, row, &retained.cells) {
                rectangles.push(RectInstance::from_pixels(
                    frame.padding + start as f32 * frame.cell_metrics.width,
                    frame.padding + row as f32 * frame.cell_metrics.height,
                    (end - start) as f32 * frame.cell_metrics.width,
                    frame.cell_metrics.height,
                    frame.selection_color,
                    0.42,
                    frame.viewport_width,
                    frame.viewport_height,
                ));
            }
        }
    }
    if frame.cursor.visible
        && (!frame.cursor.blinking || frame.cursor_blink_visible)
        && frame.cursor.row < retained.rows
        && frame.cursor.column < retained.columns
    {
        let x = frame.padding + frame.cursor.column as f32 * frame.cell_metrics.width;
        let y = frame.padding + frame.cursor.row as f32 * frame.cell_metrics.height;
        let (cursor_y, cursor_width, cursor_height) = match frame.cursor.shape {
            CursorShape::Block => (y, frame.cell_metrics.width, frame.cell_metrics.height),
            CursorShape::Underline => (
                y + frame.cell_metrics.height - 2.0,
                frame.cell_metrics.width,
                2.0,
            ),
            CursorShape::Bar => (y, 2.0, frame.cell_metrics.height),
        };
        rectangles.push(RectInstance::from_pixels(
            x,
            cursor_y,
            cursor_width,
            cursor_height,
            frame.cursor_color,
            if frame.cursor.shape == CursorShape::Block {
                0.9
            } else {
                1.0
            },
            frame.viewport_width,
            frame.viewport_height,
        ));
    }
}

#[cfg(test)]
fn build_geometry_into(
    rectangles: &mut Vec<RectInstance>,
    rounded_corners: &mut Vec<RoundedCornerInstance>,
    braille_instances: &mut Vec<BrailleInstance>,
    retained: &RetainedGrid,
    frame: RectangleFrame,
) {
    rectangles.clear();
    rounded_corners.clear();
    braille_instances.clear();
    for (row_index, plan) in retained.row_plans.iter().enumerate() {
        let y = frame.padding + row_index as f32 * frame.cell_metrics.height;
        for background in &plan.backgrounds {
            rectangles.push(RectInstance::from_pixels(
                frame.padding + background.start_column as f32 * frame.cell_metrics.width,
                y,
                (background.end_column - background.start_column) as f32 * frame.cell_metrics.width,
                frame.cell_metrics.height,
                background.color,
                1.0,
                frame.viewport_width,
                frame.viewport_height,
            ));
        }
        if let Some((start, end)) = frame
            .selection
            .and_then(|selection| selection_span_for_row(selection, row_index, &retained.cells))
        {
            rectangles.push(RectInstance::from_pixels(
                frame.padding + start as f32 * frame.cell_metrics.width,
                y,
                (end - start) as f32 * frame.cell_metrics.width,
                frame.cell_metrics.height,
                frame.selection_color,
                0.42,
                frame.viewport_width,
                frame.viewport_height,
            ));
        }
        for block in &plan.blocks {
            let cell = rounded_cell_rect(
                frame.padding,
                frame.cell_metrics.width,
                frame.cell_metrics.height,
                block.column,
                row_index,
            );
            for rectangle in block_pixel_rectangles(block.shape, cell)
                .into_iter()
                .flatten()
            {
                if rectangle.right > rectangle.left && rectangle.bottom > rectangle.top {
                    rectangles.push(RectInstance::from_pixels(
                        rectangle.left,
                        rectangle.top,
                        rectangle.right - rectangle.left,
                        rectangle.bottom - rectangle.top,
                        block.foreground,
                        1.0,
                        frame.viewport_width,
                        frame.viewport_height,
                    ));
                }
            }
        }
        for box_primitive in &plan.boxes {
            let cell = rounded_cell_rect(
                frame.padding,
                frame.cell_metrics.width,
                frame.cell_metrics.height,
                box_primitive.column,
                row_index,
            );
            if let BoxDrawing::Rounded(corner) = box_primitive.shape {
                rounded_corners.push(RoundedCornerInstance::from_pixels(
                    cell.left,
                    cell.top,
                    cell.right - cell.left,
                    cell.bottom - cell.top,
                    frame.font_size,
                    corner,
                    box_primitive.foreground,
                    frame.viewport_width,
                    frame.viewport_height,
                ));
                continue;
            }
            let metrics = BoxMetrics {
                width: cell.right - cell.left,
                height: cell.bottom - cell.top,
                font_size: frame.font_size,
            };
            for rectangle in box_rectangles(box_primitive.shape, metrics).as_slice() {
                rectangles.push(RectInstance::from_pixels(
                    cell.left + rectangle.left,
                    cell.top + rectangle.top,
                    rectangle.right - rectangle.left,
                    rectangle.bottom - rectangle.top,
                    box_primitive.foreground,
                    1.0,
                    frame.viewport_width,
                    frame.viewport_height,
                ));
            }
        }
        for braille in &plan.braille {
            if braille.pattern == 0 {
                continue;
            }
            let cell = rounded_cell_rect(
                frame.padding,
                frame.cell_metrics.width,
                frame.cell_metrics.height,
                braille.column,
                row_index,
            );
            braille_instances.push(BrailleInstance::from_pixels(
                cell.left,
                cell.top,
                cell.right - cell.left,
                cell.bottom - cell.top,
                braille.pattern,
                braille.foreground,
                frame.viewport_width,
                frame.viewport_height,
            ));
        }
        for decoration in &plan.decorations {
            let x = frame.padding + decoration.column as f32 * frame.cell_metrics.width;
            let (decoration_y, decoration_height) = match decoration.kind {
                DecorationKind::Underline => (y + frame.cell_metrics.height - 2.0, 1.0),
                DecorationKind::Strikeout => (y + frame.cell_metrics.height * 0.55, 1.0),
            };
            rectangles.push(RectInstance::from_pixels(
                x,
                decoration_y,
                frame.cell_metrics.width,
                decoration_height,
                decoration.foreground,
                1.0,
                frame.viewport_width,
                frame.viewport_height,
            ));
        }
    }
    if frame.cursor.visible
        && (!frame.cursor.blinking || frame.cursor_blink_visible)
        && frame.cursor.row < retained.rows
        && frame.cursor.column < retained.columns
    {
        let x = frame.padding + frame.cursor.column as f32 * frame.cell_metrics.width;
        let y = frame.padding + frame.cursor.row as f32 * frame.cell_metrics.height;
        let (cursor_y, cursor_width, cursor_height) = match frame.cursor.shape {
            CursorShape::Block => (y, frame.cell_metrics.width, frame.cell_metrics.height),
            CursorShape::Underline => (
                y + frame.cell_metrics.height - 2.0,
                frame.cell_metrics.width,
                2.0,
            ),
            CursorShape::Bar => (y, 2.0, frame.cell_metrics.height),
        };
        rectangles.push(RectInstance::from_pixels(
            x,
            cursor_y,
            cursor_width,
            cursor_height,
            frame.cursor_color,
            if frame.cursor.shape == CursorShape::Block {
                0.9
            } else {
                1.0
            },
            frame.viewport_width,
            frame.viewport_height,
        ));
    }
}

fn selection_span_for_row(
    selection: SelectionRange,
    row: usize,
    cells: &[Vec<Cell>],
) -> Option<(usize, usize)> {
    if cells.is_empty()
        || selection.start.row >= cells.len()
        || row < selection.start.row
        || row > selection.end.row.min(cells.len() - 1)
    {
        return None;
    }
    let cells = &cells[row];
    let columns = cells.len();
    let mut start = if row == selection.start.row {
        selection.start.column.min(columns)
    } else {
        0
    };
    let mut end = if row == selection.end.row {
        selection.end.column.saturating_add(1).min(columns)
    } else {
        columns
    };

    if start > 0
        && cells
            .get(start)
            .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER))
        && cells
            .get(start - 1)
            .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE))
    {
        start -= 1;
    }
    if end > 0
        && cells
            .get(end - 1)
            .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE))
        && cells
            .get(end)
            .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER))
    {
        end += 1;
    }
    (start < end).then_some((start, end))
}

#[cfg(test)]
fn selection_spans(selection: SelectionRange, cells: &[Vec<Cell>]) -> Vec<(usize, usize, usize)> {
    (0..cells.len())
        .filter_map(|row| {
            selection_span_for_row(selection, row, cells).map(|(start, end)| (row, start, end))
        })
        .collect()
}

const fn cursor_should_blink(cursor: CursorState, occluded: bool) -> bool {
    cursor.visible && cursor.blinking && !occluded
}

const fn redraw_after_occlusion_change(was_occluded: bool, occluded: bool) -> bool {
    was_occluded && !occluded
}

fn text_view_dimensions(
    columns: usize,
    rows: usize,
    cell_width: f32,
    cell_height: f32,
) -> (u32, u32) {
    (
        (columns as f32 * cell_width).round().max(0.0) as u32,
        (rows as f32 * cell_height).round().max(0.0) as u32,
    )
}

fn text_view_pixel_at(
    x: f64,
    y: f64,
    padding: f64,
    dimensions: (u32, u32),
) -> Option<(usize, usize)> {
    let pixel_x = (x - padding).floor();
    let pixel_y = (y - padding).floor();
    (pixel_x >= 0.0
        && pixel_y >= 0.0
        && pixel_x < f64::from(dimensions.0)
        && pixel_y < f64::from(dimensions.1))
    .then_some((pixel_x as usize, pixel_y as usize))
}

fn attrs_for_style(style: TextStyle, font_family: &str) -> Attrs<'_> {
    let mut attrs = Attrs::new()
        .family(Family::Name(font_family))
        .color(glyphon::Color::rgb(
            style.foreground[0],
            style.foreground[1],
            style.foreground[2],
        ));
    if style.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if style.italic {
        attrs = attrs.style(Style::Italic);
    }
    attrs
}

fn measure_cell(font_system: &mut FontSystem, font_size: f32) -> CellMetrics {
    let height = (font_size * 1.2).round().max(1.0);
    let mut buffer = Buffer::new(font_system, Metrics::new(font_size, height));
    buffer.set_wrap(Wrap::None);
    buffer.set_text(
        "M",
        &Attrs::new().family(Family::Monospace),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    let width = buffer
        .layout_runs()
        .next()
        .map_or(font_size * 0.6, |run| run.line_w)
        .max(1.0);
    CellMetrics { width, height }
}

fn nonzero_size(size: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(size.width.max(1), size.height.max(1))
}

fn wgpu_color(rgb: [u8; 3]) -> wgpu::Color {
    let [red, green, blue] = srgb_to_linear(rgb);
    wgpu::Color {
        r: f64::from(red),
        g: f64::from(green),
        b: f64::from(blue),
        a: 1.0,
    }
}

#[cfg(test)]
mod render_regression_tests {
    use engine::{
        Cell, CellFlags, CursorState, DEFAULT_BACKGROUND_RGB, DEFAULT_FOREGROUND_RGB, DynamicColor,
        RowMoveDirection, SelectionPoint, SelectionRange, Terminal, TerminalConfig,
    };
    use glyphon::{Attrs, Color, Family, FontSystem, Metrics, Shaping, Weight};

    use super::{
        BackgroundRun, BlockPrimitive, BlockShape, BraillePrimitive, DEFAULT_FONT_FAMILY,
        DecorationKind, DecorationPrimitive, Palette, PixelRect, RectangleFrame,
        RenderPreparationState, RenderRow, RetainedGrid, block_pixel_rectangles,
        build_geometry_into, build_search_field_geometry, cursor_should_blink,
        geometric_block_shape, geometric_braille_pattern, measure_cell,
        redraw_after_occlusion_change, rounded_cell_rect, search_field_layout, search_field_text,
        selection_spans, text_view_dimensions, text_view_pixel_at, wgpu_color,
    };

    fn eighth_coverage(shape: BlockShape) -> [u8; 8] {
        let mut coverage = [0; 8];
        for rectangle in shape.eighth_rectangles().into_iter().flatten() {
            for y in rectangle.top..rectangle.bottom {
                for x in rectangle.left..rectangle.right {
                    coverage[usize::from(y)] |= 1 << (7 - x);
                }
            }
        }
        coverage
    }

    #[test]
    fn default_background_is_linearized_for_the_srgb_surface() {
        let color = wgpu_color(DEFAULT_BACKGROUND_RGB);
        assert!(
            color.r < 0.01,
            "red channel was not converted to linear RGB"
        );
        assert!(
            color.g < 0.01,
            "green channel was not converted to linear RGB"
        );
        assert!(
            color.b < 0.01,
            "blue channel was not converted to linear RGB"
        );
    }

    #[test]
    fn search_input_is_anchored_top_right_and_keeps_the_query_tail_visible() {
        let layout = search_field_layout(1_000.0, 640.0, 1.0);
        assert!((layout.x + layout.width - 988.0).abs() < f32::EPSILON);
        assert!((layout.y - 12.0).abs() < f32::EPSILON);
        assert_eq!(
            search_field_text("needle", layout.text_width, 1.0),
            "needle"
        );

        let visible = search_field_text("a very long searchable command ending in 界面", 70.0, 1.0);
        assert!(visible.starts_with('…'));
        assert!(visible.ends_with("界面"));

        let mut rectangles = Vec::new();
        build_search_field_geometry(
            &mut rectangles,
            layout,
            Palette::default(),
            Some(false),
            Some(6),
            1_000.0,
            640.0,
        );
        assert_eq!(rectangles.len(), 7);
    }

    #[test]
    fn multiline_selection_becomes_one_clamped_rectangle_per_row() {
        let cells = vec![vec![Cell::default(); 8]; 3];
        assert_eq!(
            selection_spans(
                SelectionRange {
                    start: SelectionPoint { column: 2, row: 0 },
                    end: SelectionPoint { column: 3, row: 2 },
                },
                &cells,
            ),
            vec![(0, 2, 8), (1, 0, 8), (2, 0, 4)]
        );
    }

    #[test]
    fn metadata_only_updates_keep_prepared_text_and_viewport_clean() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 8,
            rows: 2,
            scrollback_limit: 0,
        });
        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        let mut preparation = RenderPreparationState::default();
        assert!(preparation.viewport_is_dirty());
        assert!(preparation.text_is_dirty());
        preparation.viewport_updated();
        preparation.text_prepared();

        terminal.begin_selection(SelectionPoint { column: 0, row: 0 });
        terminal.update_selection(SelectionPoint { column: 2, row: 0 });
        let selection = retained.apply_frame(
            &terminal.frame_update(false),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );
        if selection.text_changed() {
            preparation.mark_text_dirty();
        }
        assert_eq!(selection.rebuilt_rows, 0);
        assert!(!preparation.text_is_dirty());
        assert!(!preparation.viewport_is_dirty());

        terminal.feed(b"\x1b[2;2H");
        let cursor = retained.apply_frame(
            &terminal.frame_update(false),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );
        if cursor.text_changed() {
            preparation.mark_text_dirty();
        }
        assert_eq!(cursor.rebuilt_rows, 0);
        assert!(!preparation.text_is_dirty());

        terminal.feed(b"X");
        let text = retained.apply_frame(
            &terminal.frame_update(false),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );
        if text.text_changed() {
            preparation.mark_text_dirty();
        }
        assert_eq!(text.rebuilt_rows, 1);
        assert!(preparation.text_is_dirty());

        preparation.text_prepared();
        preparation.mark_viewport_dirty();
        assert!(preparation.viewport_is_dirty());
        assert!(!preparation.text_is_dirty());
    }

    #[test]
    fn unchanged_row_payloads_preserve_the_cached_generation() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 8,
            rows: 1,
            scrollback_limit: 0,
        });
        terminal.feed(b"A");
        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );
        let generation = retained.row_plans[0].generation;

        terminal.feed(b"\x1b[1;1HA");
        let unchanged = retained.apply_frame(
            &terminal.frame_update(false),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        assert_eq!(unchanged.rebuilt_rows, 0);
        assert_eq!(retained.row_plans[0].generation, generation);
    }

    #[test]
    fn ascii_fast_path_is_conservative_for_complex_and_styled_text() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 12,
            rows: 3,
            scrollback_limit: 0,
        });
        terminal.feed(b"plain ASCII");
        terminal.feed("\x1b[2;1H界e\u{301}".as_bytes());
        terminal.feed(b"\x1b[3;1H\x1b[1mbold");
        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        let update = retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        assert_eq!(update.ascii_rows_shaped, 1);
        assert_eq!(update.complex_rows_shaped, 2);
        assert!(matches!(retained.row_plans[0].shaping, Shaping::Basic));
        assert!(matches!(retained.row_plans[1].shaping, Shaping::Advanced));
        assert!(matches!(retained.row_plans[2].shaping, Shaping::Advanced));
    }

    #[test]
    fn dirty_rows_rebuild_their_retained_rectangle_plans_only() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 8,
            rows: 2,
            scrollback_limit: 0,
        });
        terminal.feed(b"\x1b[41mabc\x1b[42mde\x1b[0;4mF\x1b[0;9mG");

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        assert_eq!(
            retained.row_plans[0].backgrounds,
            vec![
                BackgroundRun {
                    start_column: 0,
                    end_column: 3,
                    color: [191, 97, 106],
                },
                BackgroundRun {
                    start_column: 3,
                    end_column: 5,
                    color: [163, 190, 140],
                },
            ]
        );
        assert_eq!(
            retained.row_plans[0].decorations,
            vec![
                DecorationPrimitive {
                    column: 5,
                    kind: DecorationKind::Underline,
                    foreground: DEFAULT_FOREGROUND_RGB,
                },
                DecorationPrimitive {
                    column: 6,
                    kind: DecorationKind::Strikeout,
                    foreground: DEFAULT_FOREGROUND_RGB,
                },
            ]
        );
        let first_row_backgrounds = retained.row_plans[0].backgrounds.clone();
        let first_row_decorations = retained.row_plans[0].decorations.clone();

        terminal.feed(b"\x1b[2;1H\x1b[44mZ");
        let update = retained.apply_frame(
            &terminal.frame_update(false),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        assert_eq!(update.rebuilt_rows, 1);
        assert_eq!(retained.row_plans[0].backgrounds, first_row_backgrounds);
        assert_eq!(retained.row_plans[0].decorations, first_row_decorations);
        assert_eq!(retained.row_plans[1].backgrounds.len(), 1);
    }

    #[test]
    fn rectangle_scratch_capacity_is_reused_after_warmup() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 8,
            rows: 2,
            scrollback_limit: 0,
        });
        terminal.feed("\x1b[41mabc\x1b[0;4mD\x1b[0m\u{2588}".as_bytes());

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell_metrics = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell_metrics.width,
            Palette::default(),
        );
        let frame = RectangleFrame {
            selection: None,
            cursor: CursorState::default(),
            cursor_blink_visible: true,
            padding: 16.0,
            cell_metrics,
            font_size: 30.0,
            viewport_width: 800.0,
            viewport_height: 600.0,
            cursor_color: [216, 222, 233],
            selection_color: [76, 110, 175],
        };
        let mut scratch = Vec::new();
        let mut corners = Vec::new();
        let mut braille = Vec::new();

        build_geometry_into(&mut scratch, &mut corners, &mut braille, &retained, frame);
        let expected_len = scratch.len();
        let warmed_capacity = scratch.capacity();
        assert!(expected_len >= 3);
        assert!(warmed_capacity >= expected_len);

        build_geometry_into(&mut scratch, &mut corners, &mut braille, &retained, frame);
        assert_eq!(scratch.len(), expected_len);
        assert_eq!(scratch.capacity(), warmed_capacity);
    }

    #[test]
    fn wide_cell_selection_highlights_both_terminal_cells() {
        let mut cells = vec![vec![Cell::default(); 4]];
        cells[0][0].flags.insert(CellFlags::WIDE);
        cells[0][1].flags.insert(CellFlags::WIDE_SPACER);

        for selection in [
            SelectionRange {
                start: SelectionPoint { column: 0, row: 0 },
                end: SelectionPoint { column: 0, row: 0 },
            },
            SelectionRange {
                start: SelectionPoint { column: 1, row: 0 },
                end: SelectionPoint { column: 1, row: 0 },
            },
        ] {
            assert_eq!(selection_spans(selection, &cells), vec![(0, 0, 2)]);
        }

        assert_eq!(
            selection_spans(
                SelectionRange {
                    start: SelectionPoint { column: 0, row: 0 },
                    end: SelectionPoint { column: 2, row: 0 },
                },
                &cells,
            ),
            vec![(0, 0, 3)]
        );
    }

    #[test]
    fn pixel_mouse_and_size_queries_share_the_unpadded_text_viewport() {
        let dimensions = text_view_dimensions(10, 5, 18.0, 36.0);
        assert_eq!(dimensions, (180, 180));
        assert_eq!(
            text_view_pixel_at(34.0, 52.0, 16.0, dimensions),
            Some((18, 36))
        );
        assert_eq!(text_view_pixel_at(15.0, 52.0, 16.0, dimensions), None);
        assert_eq!(text_view_pixel_at(196.0, 52.0, 16.0, dimensions), None);
    }

    #[test]
    fn retained_rows_align_text_and_cursor_to_the_same_cell_advance() {
        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let mut row = RenderRow::new(&mut font_system, Metrics::new(30.0, 36.0), cell.width);
        assert_eq!(row.buffer.monospace_width(), Some(cell.width));
        let prompt = "➜  tmon git:(main) ✗ clear";
        row.buffer.set_text(
            prompt,
            &Attrs::new().family(Family::Name(DEFAULT_FONT_FAMILY)),
            Shaping::Advanced,
            None,
        );
        row.buffer.shape_until_scroll(&mut font_system, false);
        let text_width = row.buffer.layout_runs().next().expect("layout run").line_w;
        let cursor_x = prompt.chars().count() as f32 * cell.width;
        assert!(
            (text_width - cursor_x).abs() < 0.01,
            "text ended at {text_width}, cursor grid ended at {cursor_x}"
        );
    }

    #[test]
    fn default_line_box_has_native_terminal_proportions() {
        let mut font_system = FontSystem::new();
        let metrics = measure_cell(&mut font_system, 30.0);
        assert!((metrics.height - 36.0).abs() < f32::EPSILON);
    }

    #[test]
    fn dynamic_foreground_rebuilds_retained_default_color_runs() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 8,
            rows: 1,
            scrollback_limit: 0,
        });
        terminal.feed(b"A\x1b[38;2;9;8;7mB");

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );
        assert_eq!(
            retained.row_plans[0].styles[0].style.foreground,
            DEFAULT_FOREGROUND_RGB
        );
        assert_eq!(retained.row_plans[0].styles[1].style.foreground, [9, 8, 7]);

        let mut palette = Palette::default();
        assert!(palette.set(DynamicColor::Foreground, [1, 2, 3]));
        retained.rebuild_all(&mut font_system, metrics, cell.width, palette);
        assert_eq!(retained.row_plans[0].styles[0].style.foreground, [1, 2, 3]);
        assert_eq!(retained.row_plans[0].styles[1].style.foreground, [9, 8, 7]);
    }

    #[test]
    fn cjk_and_emoji_overlays_do_not_shift_following_ascii_glyphs() {
        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        for (wide, label) in [("界", "CJK"), ("🙂", "emoji")] {
            let mut terminal = Terminal::new(TerminalConfig {
                columns: 8,
                rows: 1,
                scrollback_limit: 0,
            });
            terminal.feed(format!("{wide}A").as_bytes());

            let mut retained = RetainedGrid::default();
            retained.apply_frame(
                &terminal.frame_update(true),
                &mut font_system,
                metrics,
                cell.width,
                Palette::default(),
            );

            let row = &retained.row_plans[0];
            assert_eq!(row.text, "  A     ", "{label} must reserve two cells");
            let run = row.buffer.layout_runs().next().expect("wide row layout");
            let ascii = run
                .glyphs
                .iter()
                .find(|glyph| &run.text[glyph.start..glyph.end] == "A")
                .expect("following ASCII glyph");
            assert!(
                (ascii.x - cell.width * 2.0).abs() < 0.01,
                "ASCII after {label} started at {}, expected {}",
                ascii.x,
                cell.width * 2.0,
            );
            assert!(
                (run.line_w - cell.width * 8.0).abs() < 0.01,
                "eight grid cells containing {label} shaped to {}, expected {}",
                run.line_w,
                cell.width * 8.0,
            );

            assert_eq!(row.wide_glyphs.len(), 1);
            let overlay = &row.wide_glyphs[0];
            assert_eq!(overlay.column, 0);
            let overlay_run = overlay
                .buffer
                .layout_runs()
                .next()
                .expect("wide overlay layout");
            assert_eq!(overlay_run.text, wide);
            assert!(
                overlay_run.line_w.is_finite(),
                "{label} width was not finite"
            );
            assert!(
                overlay_run
                    .glyphs
                    .iter()
                    .all(|glyph| glyph.x.is_finite() && glyph.w.is_finite()),
                "{label} produced non-finite glyph geometry"
            );
        }
    }

    #[test]
    fn grok_braille_logo_does_not_shift_the_menu_label_that_follows_it() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 120,
            rows: 2,
            scrollback_limit: 0,
        });
        terminal.feed(
            concat!(
                "\x1b[1;23H⠀",
                "\x1b[1;24HNew worktree",
                "\x1b[2;24HResume session",
            )
            .as_bytes(),
        );

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        let glyph_x = |row: &RenderRow, character: &str| {
            let run = row.buffer.layout_runs().next().expect("Grok row layout");
            run.glyphs
                .iter()
                .find(|glyph| &run.text[glyph.start..glyph.end] == character)
                .map(|glyph| glyph.x)
                .expect("Grok label glyph")
        };
        let new_x = glyph_x(&retained.row_plans[0], "N");
        let resume_x = glyph_x(&retained.row_plans[1], "R");
        let expected_x = cell.width * 23.0;
        assert_eq!(
            retained.row_plans[0].braille,
            vec![BraillePrimitive {
                column: 22,
                pattern: 0,
                foreground: DEFAULT_FOREGROUND_RGB,
            }],
        );
        assert!(
            (new_x - expected_x).abs() < 0.01,
            "New worktree started at {new_x}, expected grid x {expected_x}",
        );
        assert!(
            (resume_x - expected_x).abs() < 0.01,
            "Resume session started at {resume_x}, expected grid x {expected_x}",
        );
    }

    #[test]
    fn braille_mapping_covers_every_unicode_pattern_including_blank() {
        for pattern in 0_u8..=u8::MAX {
            let character = char::from_u32(0x2800 + u32::from(pattern)).expect("Braille scalar");
            assert_eq!(
                geometric_braille_pattern(&character.to_string()),
                Some(pattern),
            );
        }
        assert_eq!(geometric_braille_pattern("A"), None);
        assert_eq!(geometric_braille_pattern("⠁⠂"), None);
    }

    #[test]
    fn dense_braille_uses_one_reused_gpu_instance_per_cell() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 120,
            rows: 1,
            scrollback_limit: 0,
        });
        terminal.feed("⣿".repeat(120).as_bytes());

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );
        let frame = RectangleFrame {
            selection: None,
            cursor: CursorState::default(),
            cursor_blink_visible: true,
            padding: 16.0,
            cell_metrics: cell,
            font_size: 30.0,
            viewport_width: 2_400.0,
            viewport_height: 200.0,
            cursor_color: [216, 222, 233],
            selection_color: [76, 110, 175],
        };
        let mut rectangles = Vec::new();
        let mut corners = Vec::new();
        let mut braille = Vec::new();

        build_geometry_into(
            &mut rectangles,
            &mut corners,
            &mut braille,
            &retained,
            frame,
        );
        assert_eq!(retained.row_plans[0].braille.len(), 120);
        assert_eq!(braille.len(), 120);
        let warmed_capacity = braille.capacity();

        build_geometry_into(
            &mut rectangles,
            &mut corners,
            &mut braille,
            &retained,
            frame,
        );
        assert_eq!(braille.len(), 120);
        assert_eq!(braille.capacity(), warmed_capacity);
    }

    #[test]
    fn opencode_logo_block_elements_fill_their_terminal_cell_geometry() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 12,
            rows: 3,
            scrollback_limit: 0,
        });
        // Exact fragments from OpenCode's logo: a regular muted left segment,
        // then bold foreground right segments containing its lower-half block.
        terminal.feed(
            "\x1b[22;38;2;128;128;128m█▀▀█\r\n\x1b[1;38;2;240;240;240m ▄ \r\n█▀▀▀".as_bytes(),
        );

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        let full = BlockShape::from_char('█').expect("full block shape");
        let upper = BlockShape::from_char('▀').expect("upper-half block shape");
        let lower = BlockShape::from_char('▄').expect("lower-half block shape");
        let expected: [&[BlockPrimitive]; 3] = [
            &[
                BlockPrimitive {
                    column: 0,
                    shape: full,
                    foreground: [128, 128, 128],
                },
                BlockPrimitive {
                    column: 1,
                    shape: upper,
                    foreground: [128, 128, 128],
                },
                BlockPrimitive {
                    column: 2,
                    shape: upper,
                    foreground: [128, 128, 128],
                },
                BlockPrimitive {
                    column: 3,
                    shape: full,
                    foreground: [128, 128, 128],
                },
            ],
            &[BlockPrimitive {
                column: 1,
                shape: lower,
                foreground: [240, 240, 240],
            }],
            &[
                BlockPrimitive {
                    column: 0,
                    shape: full,
                    foreground: [240, 240, 240],
                },
                BlockPrimitive {
                    column: 1,
                    shape: upper,
                    foreground: [240, 240, 240],
                },
                BlockPrimitive {
                    column: 2,
                    shape: upper,
                    foreground: [240, 240, 240],
                },
                BlockPrimitive {
                    column: 3,
                    shape: upper,
                    foreground: [240, 240, 240],
                },
            ],
        ];
        let padding = 8.0;
        for (row_index, (row, expected)) in retained.row_plans.iter().zip(expected).enumerate() {
            assert_eq!(row.blocks, expected);
            assert_eq!(row.text, " ".repeat(12));
            let line_width = row.buffer.layout_runs().next().expect("logo row").line_w;
            assert!((line_width - cell.width * 12.0).abs() < 0.01);

            for block in &row.blocks {
                let cell_rect =
                    rounded_cell_rect(padding, cell.width, cell.height, block.column, row_index);
                let rectangles = block_pixel_rectangles(block.shape, cell_rect)
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                let middle = cell_rect.top.midpoint(cell_rect.bottom).round();
                let expected_rect = match block.shape {
                    shape if shape == full => cell_rect,
                    shape if shape == upper => PixelRect {
                        bottom: middle,
                        ..cell_rect
                    },
                    shape if shape == lower => PixelRect {
                        top: middle,
                        ..cell_rect
                    },
                    _ => unreachable!("OpenCode fixture only uses half and full blocks"),
                };
                assert_eq!(rectangles, vec![expected_rect]);
            }
        }
    }

    #[test]
    fn tui_box_drawing_uses_retained_geometry_instead_of_font_glyphs() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 5,
            rows: 2,
            scrollback_limit: 0,
        });
        terminal.feed("┌─┬─┐\r\n└━┻━┘".as_bytes());

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        assert!(retained.row_plans.iter().all(|row| row.text == "     "));
        assert!(retained.row_plans.iter().all(|row| row.boxes.len() == 5));
        assert!(retained.row_plans.iter().all(|row| row.blocks.is_empty()));
    }

    #[test]
    fn rounded_tui_borders_are_retained_as_four_gpu_corner_instances() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 3,
            rows: 2,
            scrollback_limit: 0,
        });
        terminal.feed("╭─╮\r\n╰─╯".as_bytes());

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );
        let frame = RectangleFrame {
            selection: None,
            cursor: CursorState::default(),
            cursor_blink_visible: true,
            padding: 16.0,
            cell_metrics: cell,
            font_size: 30.0,
            viewport_width: 800.0,
            viewport_height: 600.0,
            cursor_color: [216, 222, 233],
            selection_color: [76, 110, 175],
        };
        let mut rectangles = Vec::new();
        let mut corners = Vec::new();
        let mut braille = Vec::new();
        build_geometry_into(
            &mut rectangles,
            &mut corners,
            &mut braille,
            &retained,
            frame,
        );

        assert!(retained.row_plans.iter().all(|row| row.text == "   "));
        assert_eq!(corners.len(), 4);
        let warmed_capacity = corners.capacity();
        build_geometry_into(
            &mut rectangles,
            &mut corners,
            &mut braille,
            &retained,
            frame,
        );
        assert_eq!(corners.len(), 4);
        assert_eq!(corners.capacity(), warmed_capacity);
    }

    #[test]
    fn geometric_block_mapping_covers_every_supported_fraction_and_quadrant() {
        let cases = [
            ('\u{2580}', [0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]),
            ('\u{2581}', [0, 0, 0, 0, 0, 0, 0, 0xFF]),
            ('\u{2582}', [0, 0, 0, 0, 0, 0, 0xFF, 0xFF]),
            ('\u{2583}', [0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF]),
            ('\u{2584}', [0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]),
            ('\u{2585}', [0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
            ('\u{2586}', [0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
            ('\u{2587}', [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
            ('\u{2588}', [0xFF; 8]),
            ('\u{2589}', [0xFE; 8]),
            ('\u{258A}', [0xFC; 8]),
            ('\u{258B}', [0xF8; 8]),
            ('\u{258C}', [0xF0; 8]),
            ('\u{258D}', [0xE0; 8]),
            ('\u{258E}', [0xC0; 8]),
            ('\u{258F}', [0x80; 8]),
            ('\u{2590}', [0x0F; 8]),
            ('\u{2594}', [0xFF, 0, 0, 0, 0, 0, 0, 0]),
            ('\u{2595}', [0x01; 8]),
            ('\u{2596}', [0, 0, 0, 0, 0xF0, 0xF0, 0xF0, 0xF0]),
            ('\u{2597}', [0, 0, 0, 0, 0x0F, 0x0F, 0x0F, 0x0F]),
            ('\u{2598}', [0xF0, 0xF0, 0xF0, 0xF0, 0, 0, 0, 0]),
            ('\u{2599}', [0xF0, 0xF0, 0xF0, 0xF0, 0xFF, 0xFF, 0xFF, 0xFF]),
            ('\u{259A}', [0xF0, 0xF0, 0xF0, 0xF0, 0x0F, 0x0F, 0x0F, 0x0F]),
            ('\u{259B}', [0xFF, 0xFF, 0xFF, 0xFF, 0xF0, 0xF0, 0xF0, 0xF0]),
            ('\u{259C}', [0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0x0F, 0x0F, 0x0F]),
            ('\u{259D}', [0x0F, 0x0F, 0x0F, 0x0F, 0, 0, 0, 0]),
            ('\u{259E}', [0x0F, 0x0F, 0x0F, 0x0F, 0xF0, 0xF0, 0xF0, 0xF0]),
            ('\u{259F}', [0x0F, 0x0F, 0x0F, 0x0F, 0xFF, 0xFF, 0xFF, 0xFF]),
        ];

        for (character, expected) in cases {
            let shape = BlockShape::from_char(character)
                .unwrap_or_else(|| panic!("missing U+{:04X}", u32::from(character)));
            assert_eq!(
                eighth_coverage(shape),
                expected,
                "wrong geometric coverage for U+{:04X}",
                u32::from(character)
            );
        }
        for font_shaped in ['\u{2591}', '\u{2592}', '\u{2593}'] {
            assert_eq!(BlockShape::from_char(font_shaped), None);
        }
        assert_eq!(geometric_block_shape("█\u{301}"), None);
    }

    #[test]
    fn block_rectangles_share_rounded_cell_and_fraction_edges() {
        let first = rounded_cell_rect(7.25, 18.0625, 35.75, 0, 0);
        let second = rounded_cell_rect(7.25, 18.0625, 35.75, 1, 0);
        let next_row = rounded_cell_rect(7.25, 18.0625, 35.75, 0, 1);
        assert!((first.right - second.left).abs() < f32::EPSILON);
        assert!((first.bottom - next_row.top).abs() < f32::EPSILON);

        for character in [
            '\u{2580}', '\u{2581}', '\u{2588}', '\u{2589}', '\u{2590}', '\u{2594}', '\u{2595}',
            '\u{2596}', '\u{2599}', '\u{259A}', '\u{259B}', '\u{259C}', '\u{259E}', '\u{259F}',
        ] {
            let shape = BlockShape::from_char(character).expect("geometric block");
            for rectangle in block_pixel_rectangles(shape, first).into_iter().flatten() {
                assert!(rectangle.left >= first.left && rectangle.right <= first.right);
                assert!(rectangle.top >= first.top && rectangle.bottom <= first.bottom);
                assert!(rectangle.right > rectangle.left);
                assert!(rectangle.bottom > rectangle.top);
                for edge in [
                    rectangle.left,
                    rectangle.top,
                    rectangle.right,
                    rectangle.bottom,
                ] {
                    assert!(
                        edge.fract().abs() < f32::EPSILON,
                        "U+{:04X}",
                        u32::from(character)
                    );
                }
            }
        }
    }

    #[test]
    fn block_primitives_retain_resolved_foreground_across_palette_rebuilds() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 4,
            rows: 1,
            scrollback_limit: 0,
        });
        terminal.feed("\x1b[38;2;9;8;7m█\x1b[39m▀".as_bytes());

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );
        assert_eq!(retained.row_plans[0].text, "    ");
        assert_eq!(retained.row_plans[0].blocks[0].foreground, [9, 8, 7]);
        assert_eq!(
            retained.row_plans[0].blocks[1].foreground,
            DEFAULT_FOREGROUND_RGB
        );

        let mut palette = Palette::default();
        assert!(palette.set(DynamicColor::Foreground, [1, 2, 3]));
        retained.rebuild_all(&mut font_system, metrics, cell.width, palette);
        assert_eq!(retained.row_plans[0].blocks[0].foreground, [9, 8, 7]);
        assert_eq!(retained.row_plans[0].blocks[1].foreground, [1, 2, 3]);
    }

    #[test]
    fn adjacent_wide_overlays_retain_their_nonzero_grid_columns() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 10,
            rows: 1,
            scrollback_limit: 0,
        });
        terminal.feed("x界🙂A".as_bytes());

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        let row = &retained.row_plans[0];
        assert_eq!(row.text, "x    A    ");
        assert_eq!(row.wide_glyphs.len(), 1);
        assert_eq!(row.wide_glyphs[0].column, 1);
        assert_eq!(row.wide_glyphs[0].columns, 4);
        assert_eq!(
            row.wide_glyphs[0]
                .buffer
                .layout_runs()
                .next()
                .expect("adjacent wide overlay")
                .text,
            "界🙂"
        );
        let run = row.buffer.layout_runs().next().expect("adjacent wide row");
        let ascii = run
            .glyphs
            .iter()
            .find(|glyph| &run.text[glyph.start..glyph.end] == "A")
            .expect("ASCII following adjacent wide glyphs");
        assert!((ascii.x - cell.width * 5.0).abs() < 0.01);
    }

    #[test]
    fn combining_mark_on_a_wide_cell_keeps_overlay_geometry_finite() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 6,
            rows: 1,
            scrollback_limit: 0,
        });
        terminal.feed("界\u{301}A".as_bytes());

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        let row = &retained.row_plans[0];
        assert_eq!(row.wide_glyphs.len(), 1);
        let overlay_run = row.wide_glyphs[0]
            .buffer
            .layout_runs()
            .next()
            .expect("combined wide overlay");
        assert_eq!(overlay_run.text, "界\u{301}");
        assert!(overlay_run.line_w.is_finite());
        assert!(overlay_run.glyphs.iter().all(|glyph| {
            glyph.x.is_finite()
                && glyph.w.is_finite()
                && glyph.x_offset.is_finite()
                && glyph.y_offset.is_finite()
        }));

        let row_run = row.buffer.layout_runs().next().expect("combined wide row");
        let ascii = row_run
            .glyphs
            .iter()
            .find(|glyph| &row_run.text[glyph.start..glyph.end] == "A")
            .expect("ASCII after combined wide glyph");
        assert!((ascii.x - cell.width * 2.0).abs() < 0.01);
    }

    #[test]
    fn wide_overlay_preserves_bold_and_explicit_color() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 4,
            rows: 1,
            scrollback_limit: 0,
        });
        terminal.feed("\x1b[1;38;2;9;8;7m界".as_bytes());

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        let run = retained.row_plans[0].wide_glyphs[0]
            .buffer
            .layout_runs()
            .next()
            .expect("styled wide overlay");
        assert!(run.glyphs.iter().all(|glyph| {
            glyph.font_weight == Weight::BOLD && glyph.color_opt == Some(Color::rgb(9, 8, 7))
        }));
    }

    #[test]
    fn bold_ascii_runs_keep_the_grid_cell_advance() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 5,
            rows: 1,
            scrollback_limit: 0,
        });
        terminal.feed(b"A\x1b[1mB\x1b[22mC");

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        let run = retained.row_plans[0]
            .buffer
            .layout_runs()
            .next()
            .expect("mixed weight row");
        for (character, column) in [("A", 0), ("B", 1), ("C", 2)] {
            let glyph = run
                .glyphs
                .iter()
                .find(|glyph| &run.text[glyph.start..glyph.end] == character)
                .expect("ASCII glyph");
            assert!(
                (glyph.x - column as f32 * cell.width).abs() < 0.01,
                "{character} started at {}, expected grid column {column}",
                glyph.x
            );
        }
    }

    #[test]
    fn only_visible_blinking_cursors_schedule_blink_frames() {
        let steady = CursorState {
            visible: true,
            blinking: false,
            ..CursorState::default()
        };
        assert!(!cursor_should_blink(steady, false));

        let blinking = CursorState {
            blinking: true,
            ..steady
        };
        assert!(cursor_should_blink(blinking, false));
        assert!(!cursor_should_blink(blinking, true));
        assert!(!cursor_should_blink(CursorState::default(), false));
    }

    #[test]
    fn returning_from_occlusion_requests_a_replacement_for_skipped_frames() {
        assert!(redraw_after_occlusion_change(true, false));
        assert!(!redraw_after_occlusion_change(false, false));
        assert!(!redraw_after_occlusion_change(false, true));
        assert!(!redraw_after_occlusion_change(true, true));
    }

    #[test]
    fn history_scroll_only_reshapes_exposed_rows_in_both_directions() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 16,
            rows: 4,
            scrollback_limit: 100,
        });
        let _ = terminal.frame_update(true);
        for index in 0..20 {
            terminal.feed(format!("line {index:02}\r\n").as_bytes());
        }

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        let initial = terminal.frame_update(true);
        retained.apply_frame(
            &initial,
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        terminal.scroll_display(1);
        let scroll = terminal.frame_update(false);
        assert!(
            !scroll.full,
            "a short history scroll should stay incremental"
        );
        assert_eq!(scroll.row_moves.len(), 1);
        assert_eq!(scroll.row_moves[0].direction, RowMoveDirection::Down);
        assert_eq!(scroll.row_moves[0].count, 1);
        assert_eq!(scroll.row_updates.len(), 1);
        let shaped = retained
            .apply_frame(
                &scroll,
                &mut font_system,
                metrics,
                cell.width,
                Palette::default(),
            )
            .rebuilt_rows;
        assert!(
            shaped <= 1,
            "one exposed row should be shaped, but {shaped} rows were shaped"
        );

        terminal.scroll_display(3);
        let scroll = terminal.frame_update(false);
        let shaped = retained
            .apply_frame(
                &scroll,
                &mut font_system,
                metrics,
                cell.width,
                Palette::default(),
            )
            .rebuilt_rows;
        assert!(
            shaped <= 3,
            "three exposed rows should be shaped, but {shaped} rows were shaped"
        );

        terminal.scroll_display(-2);
        let scroll = terminal.frame_update(false);
        let shaped = retained
            .apply_frame(
                &scroll,
                &mut font_system,
                metrics,
                cell.width,
                Palette::default(),
            )
            .rebuilt_rows;
        assert!(
            shaped <= 2,
            "two rows exposed in the opposite direction should be shaped, but {shaped} were shaped"
        );

        terminal.scroll_display(-10_000);
        let scroll = terminal.frame_update(false);
        retained.apply_frame(
            &scroll,
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );
        terminal.scroll_display(-1);
        let clamped = terminal.frame_update(false);
        assert!(
            !clamped.has_damage(),
            "a clamped scroll must not request a redundant GPU frame"
        );
        let shaped = retained
            .apply_frame(
                &clamped,
                &mut font_system,
                metrics,
                cell.width,
                Palette::default(),
            )
            .rebuilt_rows;
        assert_eq!(
            shaped, 0,
            "a clamped scroll must not reshape unchanged rows"
        );
    }

    #[test]
    fn widening_the_grid_reuses_unchanged_row_shapes() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 16,
            rows: 4,
            scrollback_limit: 100,
        });
        terminal.feed(b"unchanged");

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        let initial = terminal.frame_update(true);
        retained.apply_frame(
            &initial,
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        terminal.resize(17, 4);
        let resize = terminal.frame_update(false);
        let shaped = retained
            .apply_frame(
                &resize,
                &mut font_system,
                metrics,
                cell.width,
                Palette::default(),
            )
            .rebuilt_rows;

        assert_eq!(
            shaped, 0,
            "adding a blank column must not discard unchanged glyph layouts"
        );
    }

    #[test]
    fn shrinking_the_grid_reshapes_a_row_with_truncated_content() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 16,
            rows: 4,
            scrollback_limit: 100,
        });
        terminal.feed(b"\x1b[16GX");

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        let initial = terminal.frame_update(true);
        retained.apply_frame(
            &initial,
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        terminal.resize(15, 4);
        let resize = terminal.frame_update(false);
        let shaped = retained
            .apply_frame(
                &resize,
                &mut font_system,
                metrics,
                cell.width,
                Palette::default(),
            )
            .rebuilt_rows;

        assert_eq!(
            shaped, 1,
            "the row whose visible tail was truncated must be reshaped"
        );
    }

    #[test]
    #[ignore = "manual dense CJK retained-row benchmark"]
    fn retained_dense_cjk_benchmark() {
        use std::{hint::black_box, time::Instant};

        const COLUMNS: usize = 120;
        const ROWS: usize = 40;
        const FRAMES: usize = 50;

        let dense = "界".repeat(COLUMNS / 2);
        let alternate = "語".repeat(COLUMNS / 2);
        let mut terminal = Terminal::new(TerminalConfig {
            columns: COLUMNS,
            rows: ROWS,
            scrollback_limit: 0,
        });
        for row in 1..=ROWS {
            terminal.feed(format!("\x1b[{row};1H{dense}").as_bytes());
        }

        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .set_monospace_family(DEFAULT_FONT_FAMILY);
        let cell = measure_cell(&mut font_system, 30.0);
        let metrics = Metrics::new(30.0, 36.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &terminal.frame_update(true),
            &mut font_system,
            metrics,
            cell.width,
            Palette::default(),
        );

        let started = Instant::now();
        let mut shaped_rows = 0;
        for frame in 0..FRAMES {
            let line = if frame % 2 == 0 { &alternate } else { &dense };
            terminal.feed(format!("\x1b[1;1H{line}").as_bytes());
            shaped_rows += retained
                .apply_frame(
                    &terminal.frame_update(false),
                    &mut font_system,
                    metrics,
                    cell.width,
                    Palette::default(),
                )
                .rebuilt_rows;
            black_box(&retained);
        }
        let elapsed = started.elapsed();

        println!(
            "dense CJK retained renderer: {:.3} ms/frame ({shaped_rows} rows, {} overlays/row)",
            elapsed.as_secs_f64() * 1_000.0 / FRAMES as f64,
            retained.row_plans[0].wide_glyphs.len(),
        );
        assert_eq!(shaped_rows, FRAMES);
        assert_eq!(retained.row_plans[0].wide_glyphs.len(), 1);
        assert_eq!(retained.row_plans[0].wide_glyphs[0].columns, COLUMNS);
    }

    #[test]
    #[ignore = "manual renderer latency benchmark"]
    fn retained_history_scroll_benchmark() {
        use std::{hint::black_box, time::Instant};

        const COLUMNS: usize = 120;
        const ROWS: usize = 40;
        const STEPS: usize = 100;

        let mut terminal = Terminal::new(TerminalConfig {
            columns: COLUMNS,
            rows: ROWS,
            scrollback_limit: 1_000,
        });
        let _ = terminal.frame_update(true);
        for index in 0..400 {
            terminal.feed(format!("history row {index:04}\r\n").as_bytes());
        }
        let initial = terminal.frame_update(true);
        let mut updates = Vec::with_capacity(STEPS);
        for step in 0..STEPS {
            terminal.scroll_display(if step.is_multiple_of(2) { 1 } else { -1 });
            updates.push(terminal.frame_update(false));
        }

        let mut baseline_terminal = Terminal::new(TerminalConfig {
            columns: COLUMNS,
            rows: ROWS,
            scrollback_limit: 1_000,
        });
        let _ = baseline_terminal.frame_update(true);
        for index in 0..400 {
            baseline_terminal.feed(format!("history row {index:04}\r\n").as_bytes());
        }
        let _ = baseline_terminal.frame_update(true);
        let mut full_updates = Vec::with_capacity(STEPS);
        for step in 0..STEPS {
            baseline_terminal.scroll_display(if step.is_multiple_of(2) { 1 } else { -1 });
            full_updates.push(baseline_terminal.frame_update(true));
        }

        let new_font_system = || {
            let mut font_system = FontSystem::new();
            font_system
                .db_mut()
                .set_monospace_family(DEFAULT_FONT_FAMILY);
            font_system
        };
        let metrics = Metrics::new(30.0, 36.0);

        let mut baseline_font_system = new_font_system();
        let baseline_cell = measure_cell(&mut baseline_font_system, 30.0);
        let started = Instant::now();
        let mut baseline_shapes = 0;
        for update in &full_updates {
            let mut discarded = RetainedGrid::default();
            baseline_shapes += discarded
                .apply_frame(
                    update,
                    &mut baseline_font_system,
                    metrics,
                    baseline_cell.width,
                    Palette::default(),
                )
                .rebuilt_rows;
            black_box(discarded);
        }
        let baseline_elapsed = started.elapsed();

        let mut retained_font_system = new_font_system();
        let retained_cell = measure_cell(&mut retained_font_system, 30.0);
        let mut retained = RetainedGrid::default();
        retained.apply_frame(
            &initial,
            &mut retained_font_system,
            metrics,
            retained_cell.width,
            Palette::default(),
        );
        let started = Instant::now();
        let mut retained_shapes = 0;
        for update in &updates {
            retained_shapes += retained
                .apply_frame(
                    update,
                    &mut retained_font_system,
                    metrics,
                    retained_cell.width,
                    Palette::default(),
                )
                .rebuilt_rows;
            black_box(&retained);
        }
        let retained_elapsed = started.elapsed();

        println!(
            "scroll renderer: baseline {:.3} ms/step ({baseline_shapes} shapes), retained {:.3} ms/step ({retained_shapes} shapes)",
            baseline_elapsed.as_secs_f64() * 1_000.0 / STEPS as f64,
            retained_elapsed.as_secs_f64() * 1_000.0 / STEPS as f64,
        );
        assert!(baseline_shapes >= (ROWS - 1) * STEPS);
        assert_eq!(retained_shapes, STEPS);
        assert!(retained_elapsed < baseline_elapsed);
    }
}
