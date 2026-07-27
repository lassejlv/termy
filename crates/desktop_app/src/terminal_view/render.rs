use super::scrollbar as terminal_scrollbar;
use super::*;
use crate::ui::scrollbar::{self as ui_scrollbar, ScrollbarPaintStyle};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use gpui::prelude::FluentBuilder;
use gpui::{ElementInputHandler, ObjectFit, StyledImage, canvas};
use std::sync::Arc;
use std::time::Instant;
#[cfg(debug_assertions)]
use termy_terminal_ui::terminal_ui_render_metrics_snapshot;
use termy_terminal_ui::{add_span_damage_compute_us, terminal_ui_render_metrics_enabled};

#[derive(Clone, Copy)]
struct KittyGraphicsSelectionPaint<'a> {
    pane_id: &'a str,
    display_offset: usize,
    selection_range: Option<(SelectionPos, SelectionPos)>,
    explicit: Option<&'a KittyImageSelection>,
    color: gpui::Rgba,
}

fn kitty_graphics_layers(
    mut placements: Vec<KittyGraphicsRenderPlacement>,
    cell_size: Size<Pixels>,
    image_cache: &mut HashMap<(u32, u64), Arc<gpui::Image>>,
    selection: KittyGraphicsSelectionPaint<'_>,
) -> (Vec<AnyElement>, Vec<AnyElement>) {
    placements.sort_by_key(|placement| {
        (
            placement.z_index,
            placement.image_id,
            placement.placement_id,
            placement.placement_serial,
        )
    });
    let mut below_text = Vec::new();
    let mut above_text = Vec::new();
    let cell_width: f32 = cell_size.width.into();
    let cell_height: f32 = cell_size.height.into();
    let active_images: HashSet<(u32, u64)> = placements
        .iter()
        .map(|placement| (placement.image_id, placement.image_generation))
        .collect();
    image_cache.retain(|key, _| active_images.contains(key));

    for placement in placements {
        let bounds = kitty_graphics_placement_bounds(&placement, cell_width, cell_height);
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            continue;
        }

        let scale_x = bounds.width / placement.source_width.max(1) as f32;
        let scale_y = bounds.height / placement.source_height.max(1) as f32;
        let image = image_cache
            .entry((placement.image_id, placement.image_generation))
            .or_insert_with(|| {
                Arc::new(gpui::Image::from_bytes(
                    gpui::ImageFormat::Png,
                    placement.png.as_ref().to_vec(),
                ))
            })
            .clone();
        let image_element = gpui::img(image)
            .absolute()
            .left(px(-(placement.source_x as f32 * scale_x)))
            .top(px(-(placement.source_y as f32 * scale_y)))
            .w(px(placement.image_width as f32 * scale_x))
            .h(px(placement.image_height as f32 * scale_y))
            .object_fit(ObjectFit::Fill);
        let selected = selection
            .explicit
            .is_some_and(|selected| selected.matches(selection.pane_id, &placement))
            || kitty_graphics_placement_intersects_selection(
                &placement,
                selection.display_offset,
                selection.selection_range,
            );
        let mut selection_tint = selection.color;
        selection_tint.a = 0.22;
        let mut selection_border = selection.color;
        selection_border.a = 0.92;
        let layer = div()
            .absolute()
            .left(px(bounds.left))
            .top(px(bounds.top))
            .w(px(bounds.width))
            .h(px(bounds.height))
            .overflow_hidden()
            .child(image_element)
            .when(selected, |layer| {
                layer.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .bg(selection_tint)
                        .border_1()
                        .border_color(selection_border),
                )
            })
            .into_any_element();
        if placement.z_index < 0 {
            below_text.push(layer);
        } else {
            above_text.push(layer);
        }
    }
    (below_text, above_text)
}

fn blend_rgb_only(base: gpui::Rgba, target: gpui::Rgba, factor: f32) -> gpui::Rgba {
    let factor = factor.clamp(0.0, 1.0);
    let inv = 1.0 - factor;
    gpui::Rgba {
        r: (base.r * inv) + (target.r * factor),
        g: (base.g * inv) + (target.g * factor),
        b: (base.b * inv) + (target.b * factor),
        a: base.a,
    }
}

fn desaturate_rgb(color: gpui::Rgba, amount: f32) -> gpui::Rgba {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= f32::EPSILON {
        return color;
    }
    let luma = (color.r * 0.2126) + (color.g * 0.7152) + (color.b * 0.0722);
    let inv = 1.0 - amount;
    gpui::Rgba {
        r: (color.r * inv) + (luma * amount),
        g: (color.g * inv) + (luma * amount),
        b: (color.b * inv) + (luma * amount),
        a: color.a,
    }
}

const COMMAND_PALETTE_BACKDROP_STRENGTH: f32 = 1.0;
const TERMINAL_PROGRESS_LOADER_HEIGHT: f32 = 2.0;
const TERMINAL_PROGRESS_INDETERMINATE_WIDTH: f32 = 0.22;
const TERMINAL_PROGRESS_INDETERMINATE_CYCLE_MS: u128 = 3_400;

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalProgressLoaderFill {
    left_ratio: f32,
    width_ratio: f32,
}

fn terminal_progress_loader_fill(
    state: ProgressState,
    elapsed_ms: u128,
) -> Option<TerminalProgressLoaderFill> {
    if !state.is_active() {
        return None;
    }

    if let Some(percent) = state.percentage() {
        return Some(TerminalProgressLoaderFill {
            left_ratio: 0.0,
            width_ratio: (f32::from(percent) / 100.0).clamp(0.0, 1.0),
        });
    }

    let cycle_position = (elapsed_ms % TERMINAL_PROGRESS_INDETERMINATE_CYCLE_MS) as f32
        / TERMINAL_PROGRESS_INDETERMINATE_CYCLE_MS as f32;
    let directional_progress = if cycle_position <= 0.5 {
        cycle_position * 2.0
    } else {
        (1.0 - cycle_position) * 2.0
    };
    let eased_progress = smoothstep(directional_progress);
    Some(TerminalProgressLoaderFill {
        left_ratio: eased_progress * (1.0 - TERMINAL_PROGRESS_INDETERMINATE_WIDTH),
        width_ratio: TERMINAL_PROGRESS_INDETERMINATE_WIDTH,
    })
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct UpdateBannerLayout {
    overlay_top: f32,
    overlay_left: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CellColorTransform {
    fg_blend: f32,
    bg_blend: f32,
    desaturate: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CellTextAttributes {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

fn cell_text_attributes(flags: Flags) -> CellTextAttributes {
    CellTextAttributes {
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        strikethrough: flags.contains(Flags::STRIKEOUT),
    }
}

impl CellColorTransform {
    fn is_active(self) -> bool {
        self.fg_blend > f32::EPSILON
            || self.bg_blend > f32::EPSILON
            || self.desaturate > f32::EPSILON
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneCacheUpdateStrategy {
    Reuse,
    Partial,
    Full,
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RenderPassCacheStrategyCounts {
    full: u64,
    partial: u64,
    reuse: u64,
    dirty_span_count: u64,
    patched_cell_count: u64,
}

#[cfg(debug_assertions)]
impl RenderPassCacheStrategyCounts {
    fn record(&mut self, strategy: PaneCacheUpdateStrategy) {
        match strategy {
            PaneCacheUpdateStrategy::Reuse => {
                self.reuse = self.reuse.saturating_add(1);
            }
            PaneCacheUpdateStrategy::Partial => {
                self.partial = self.partial.saturating_add(1);
            }
            PaneCacheUpdateStrategy::Full => {
                self.full = self.full.saturating_add(1);
            }
        }
    }

    fn record_partial_work(&mut self, dirty_span_count: usize, patched_cell_count: usize) {
        self.dirty_span_count = self
            .dirty_span_count
            .saturating_add(usize_to_u64_saturating(dirty_span_count));
        self.patched_cell_count = self
            .patched_cell_count
            .saturating_add(usize_to_u64_saturating(patched_cell_count));
    }
}

#[cfg(debug_assertions)]
fn increment_render_count_counter(counters: &mut TerminalRenderMetricsCounters) {
    counters.render_count = counters.render_count.saturating_add(1);
}

#[cfg(debug_assertions)]
fn usize_to_u64_saturating(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn pane_cache_update_strategy(
    cache_has_cells: bool,
    cache_size_matches: bool,
    cache_offset_matches: bool,
    cache_key_matches: bool,
    damage: &TerminalDamageSnapshot,
) -> PaneCacheUpdateStrategy {
    if !cache_has_cells || !cache_size_matches || !cache_offset_matches || !cache_key_matches {
        return PaneCacheUpdateStrategy::Full;
    }
    match damage {
        TerminalDamageSnapshot::Full => PaneCacheUpdateStrategy::Full,
        TerminalDamageSnapshot::Partial(spans) if spans.is_empty() => {
            PaneCacheUpdateStrategy::Reuse
        }
        TerminalDamageSnapshot::Partial(_) => PaneCacheUpdateStrategy::Partial,
    }
}

fn finalized_cache_update_strategy(
    planned: PaneCacheUpdateStrategy,
    did_full_rebuild: bool,
) -> PaneCacheUpdateStrategy {
    if planned == PaneCacheUpdateStrategy::Partial && did_full_rebuild {
        PaneCacheUpdateStrategy::Full
    } else {
        planned
    }
}

thread_local! {
    static DIRTY_SPAN_RANGES: std::cell::RefCell<Vec<(usize, usize, usize)>> =
        std::cell::RefCell::new(Vec::with_capacity(128));
}

fn dirty_span_cell_upper_bound(spans: &[TerminalDirtySpan], rows: usize, cols: usize) -> usize {
    if rows == 0 || cols == 0 {
        return 0;
    }

    spans.iter().fold(0usize, |acc, span| {
        if span.row >= rows {
            return acc;
        }
        let left = span.left_col.min(cols.saturating_sub(1));
        let right = span.right_col.min(cols.saturating_sub(1));
        if left > right {
            return acc;
        }
        acc.saturating_add(right.saturating_sub(left).saturating_add(1))
    })
}

fn partial_damage_should_rebuild_full(
    spans: &[TerminalDirtySpan],
    rows: usize,
    cols: usize,
) -> bool {
    let total_cells = rows.saturating_mul(cols);
    if total_cells == 0 {
        return false;
    }

    dirty_span_cell_upper_bound(spans, rows, cols) >= total_cells / 2
}

fn paint_damage_from_dirty_spans(
    spans: &[TerminalDirtySpan],
    row_count: usize,
) -> TerminalGridPaintDamage {
    let metrics_started_at = terminal_ui_render_metrics_enabled().then(Instant::now);
    let result = if spans.is_empty() {
        TerminalGridPaintDamage::None
    } else if let [span] = spans {
        if span.row < row_count {
            TerminalGridPaintDamage::RowRanges(Arc::from([(
                span.row,
                span.left_col,
                span.right_col,
            )]))
        } else {
            TerminalGridPaintDamage::None
        }
    } else {
        DIRTY_SPAN_RANGES.with(|buf| {
            let mut ranges = buf.borrow_mut();
            ranges.clear();
            for span in spans {
                if span.row < row_count {
                    ranges.push((span.row, span.left_col, span.right_col));
                }
            }
            if ranges.is_empty() {
                return TerminalGridPaintDamage::None;
            }
            // Sort by row/column so entries for the same row are adjacent and the
            // first item on a row owns the full left bound after merging.
            ranges.sort_unstable_by_key(|&(row, left_col, _)| (row, left_col));
            ranges.dedup_by(|b, a| {
                if a.0 == b.0 {
                    a.1 = a.1.min(b.1);
                    a.2 = a.2.max(b.2);
                    true
                } else {
                    false
                }
            });
            TerminalGridPaintDamage::RowRanges(Arc::from(ranges.as_slice()))
        })
    };
    if let Some(started_at) = metrics_started_at {
        add_span_damage_compute_us(started_at.elapsed().as_micros() as u64);
    }
    result
}

#[derive(Clone, Copy)]
struct PaneCellBuildContext<'a> {
    colors: &'a TerminalColors,
    effective_background_opacity: f32,
    background_opacity_cells: bool,
    cell_color_transform: CellColorTransform,
    pane_focus_target_bg: gpui::Rgba,
    terminal_surface_bg: gpui::Rgba,
    selection_range: Option<(SelectionPos, SelectionPos)>,
    pane_search_results: Option<&'a termy_search::SearchResults>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResolvedCellColors {
    fg: gpui::Rgba,
    bg: gpui::Rgba,
    uses_terminal_default_bg: bool,
}

fn uses_terminal_default_background(color: AnsiColor) -> bool {
    matches!(color, AnsiColor::Named(NamedColor::Background))
}

fn uses_block_element_background(c: char) -> bool {
    matches!(c as u32, 0x2580..=0x259F)
}

fn resolved_default_cell_colors(context: PaneCellBuildContext<'_>) -> (gpui::Rgba, gpui::Rgba) {
    let mut default_bg = context.colors.background;
    default_bg.a *= context.effective_background_opacity;
    apply_cell_color_transform(
        context.colors.foreground,
        default_bg,
        context.cell_color_transform,
        context.pane_focus_target_bg,
        context.terminal_surface_bg,
    )
}

fn resolve_cell_colors(
    cell_content: &alacritty_terminal::term::cell::Cell,
    context: PaneCellBuildContext<'_>,
) -> ResolvedCellColors {
    let mut fg_source = cell_content.fg;
    let mut bg_source = cell_content.bg;
    if cell_content.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg_source, &mut bg_source);
    }

    // Decide transparency from the terminal color source, not the resolved RGB.
    // Block-element workloads like doom fire encode visible pixels in the cell
    // background, so those explicit backgrounds must stay opaque even when they
    // numerically match the theme background.
    let mut fg = context.colors.convert(fg_source);
    let mut bg = context.colors.convert(bg_source);
    if cell_content.flags.contains(Flags::DIM) {
        fg.r *= DIM_TEXT_FACTOR;
        fg.g *= DIM_TEXT_FACTOR;
        fg.b *= DIM_TEXT_FACTOR;
    }
    let uses_terminal_default_bg = uses_terminal_default_background(bg_source);
    let apply_background_opacity = uses_terminal_default_bg
        || (context.background_opacity_cells && !uses_block_element_background(cell_content.c));
    if apply_background_opacity {
        bg.a *= context.effective_background_opacity;
    }
    (fg, bg) = apply_cell_color_transform(
        fg,
        bg,
        context.cell_color_transform,
        context.pane_focus_target_bg,
        context.terminal_surface_bg,
    );

    ResolvedCellColors {
        fg,
        bg,
        uses_terminal_default_bg,
    }
}

fn selection_range_contains(
    selection_range: Option<(SelectionPos, SelectionPos)>,
    col: usize,
    line: i32,
) -> bool {
    let Some((start, end)) = selection_range else {
        return false;
    };
    let here = (line, col);
    here >= (start.line, start.col) && here <= (end.line, end.col)
}

fn term_line_from_viewport_row(row: usize, display_offset: usize) -> Option<i32> {
    let row = i64::try_from(row).ok()?;
    let display_offset = i64::try_from(display_offset).ok()?;
    i32::try_from(row - display_offset).ok()
}

fn filtered_cursor_state(
    cursor_state: Option<TerminalCursorState>,
    pane_display_offset: usize,
    is_active_pane: bool,
    cols: usize,
    rows: usize,
) -> Option<TerminalCursorState> {
    cursor_state
        .filter(|_| pane_display_offset == 0 && is_active_pane)
        .filter(|cursor| cursor.col < cols && cursor.row < rows)
}

fn cursor_state_for_pane(
    terminal: &Terminal,
    pane_display_offset: usize,
    is_active_pane: bool,
    cols: usize,
    rows: usize,
) -> Option<TerminalCursorState> {
    filtered_cursor_state(
        terminal.cursor_state(),
        pane_display_offset,
        is_active_pane,
        cols,
        rows,
    )
}

fn cursor_state_with_preview(
    preview: Option<&PendingCursorMovePreview>,
    pane_id: &str,
    actual: Option<TerminalCursorState>,
    is_active_pane: bool,
    cols: usize,
    rows: usize,
) -> Option<TerminalCursorState> {
    let Some(preview) = preview else {
        return actual;
    };
    if !is_active_pane
        || preview.pane_id != pane_id
        || preview.target.col >= cols
        || preview.target.row >= rows
    {
        return actual;
    }

    match actual {
        Some(cursor) if cursor.col == preview.target.col && cursor.row == preview.target.row => {
            Some(cursor)
        }
        _ => Some(TerminalCursorState {
            col: preview.target.col,
            row: preview.target.row,
            style: preview.style,
        }),
    }
}

type PaneRenderCells = TerminalGridRows;

fn pane_render_cells_match_dimensions(cells: &PaneRenderCells, cols: usize, rows: usize) -> bool {
    cells.len() == rows && cells.iter().all(|row_cells| row_cells.len() == cols)
}

fn patch_pane_render_row(
    cells: &mut [Arc<Vec<CellRenderInfo>>],
    rows: usize,
    cols: usize,
    row: usize,
    left_col: usize,
    right_col: usize,
    mut build_cell: impl FnMut(usize) -> CellRenderInfo,
) -> usize {
    if row >= rows || left_col >= cols || left_col > right_col {
        return 0;
    }
    let Some(row_cells) = cells.get_mut(row) else {
        return 0;
    };
    if row_cells.len() != cols {
        return 0;
    }

    let row_cells = Arc::make_mut(row_cells);
    let right_col = right_col.min(cols.saturating_sub(1));
    for col in left_col..=right_col {
        row_cells[col] = build_cell(col);
    }
    right_col.saturating_sub(left_col).saturating_add(1)
}

fn command_palette_backdrop_transform() -> CellColorTransform {
    let preset = pane_focus_preset(PaneFocusEffect::SoftSpotlight)
        .expect("soft spotlight pane focus preset must exist");
    CellColorTransform {
        fg_blend: preset.inactive_fg_blend * COMMAND_PALETTE_BACKDROP_STRENGTH,
        bg_blend: preset.inactive_bg_blend * COMMAND_PALETTE_BACKDROP_STRENGTH,
        desaturate: preset.inactive_desaturate * COMMAND_PALETTE_BACKDROP_STRENGTH,
    }
}

fn apply_cell_color_transform(
    fg: gpui::Rgba,
    bg: gpui::Rgba,
    transform: CellColorTransform,
    fg_blend_target: gpui::Rgba,
    bg_blend_target: gpui::Rgba,
) -> (gpui::Rgba, gpui::Rgba) {
    if !transform.is_active() {
        return (fg, bg);
    }

    let mut next_fg = fg;
    let mut next_bg = bg;
    if transform.fg_blend > f32::EPSILON {
        next_fg = blend_rgb_only(next_fg, fg_blend_target, transform.fg_blend);
    }
    if transform.bg_blend > f32::EPSILON {
        next_bg = blend_rgb_only(next_bg, bg_blend_target, transform.bg_blend);
    }
    if transform.desaturate > f32::EPSILON {
        next_fg = desaturate_rgb(next_fg, transform.desaturate);
        next_bg = desaturate_rgb(next_bg, transform.desaturate);
    }
    (next_fg, next_bg)
}

fn effective_pane_focus_active_border_alpha(
    active_border_alpha: f32,
    runtime_uses_tmux: bool,
    tmux_show_active_pane_border: bool,
) -> f32 {
    if runtime_uses_tmux && !tmux_show_active_pane_border {
        return 0.0;
    }
    active_border_alpha
}

fn pane_focus_factors(is_active_pane: bool, pane_focus_enabled: bool) -> (f32, f32) {
    if !pane_focus_enabled {
        return (0.0, 0.0);
    }

    if is_active_pane {
        (0.0, 1.0)
    } else {
        (1.0, 0.0)
    }
}

fn terminal_scrollbar_overlay_frame(
    surface: TerminalScrollbarSurfaceGeometry,
) -> Option<TerminalScrollbarGutterFrame> {
    surface.gutter_frame()
}

fn terminal_scrollbar_track_width(frame_width: f32) -> f32 {
    TERMINAL_SCROLLBAR_TRACK_WIDTH.min(frame_width.max(0.0))
}

#[cfg(any(not(target_os = "macos"), test))]
fn context_menu_visible_height(
    content_height: f32,
    viewport_height: Option<f32>,
    row_height: f32,
) -> f32 {
    viewport_height
        .map(|height| content_height.min((height - 16.0).max(row_height + 8.0)))
        .unwrap_or(content_height)
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    fn update_banner_layout_for(
        show_update_banner: bool,
        show_tab_strip_chrome: bool,
    ) -> Option<UpdateBannerLayout> {
        if !show_update_banner {
            return None;
        }

        Some(UpdateBannerLayout {
            overlay_top: Self::window_titlebar_height_for(false, show_tab_strip_chrome) + 10.0,
            overlay_left: 0.0,
        })
    }

    fn update_banner_layout(&self) -> Option<UpdateBannerLayout> {
        Self::update_banner_layout_for(
            self.update_banner_visible(),
            self.should_render_tab_strip_chrome(),
        )
    }
    fn pane_render_cache_key(
        &self,
        is_active_pane: bool,
        alternate_screen_mode: bool,
        search_active: bool,
        cell_color_transform: CellColorTransform,
        effective_background_opacity: f32,
    ) -> TerminalPaneRenderCacheKey {
        let (search_results_revision, search_position) = if search_active && is_active_pane {
            let results = self.search_state.results();
            (
                Some(self.search_state.results_revision()),
                results.position(),
            )
        } else {
            (None, None)
        };

        TerminalPaneRenderCacheKey {
            is_active_pane,
            alternate_screen_mode,
            selection_range: is_active_pane.then(|| self.selection_range()).flatten(),
            search_results_revision,
            search_position,
            effective_background_opacity_bits: effective_background_opacity.to_bits(),
            background_opacity_cells: self.background_opacity_cells,
            color_transform: TerminalPaneCellColorTransformKey {
                fg_blend_bits: cell_color_transform.fg_blend.to_bits(),
                bg_blend_bits: cell_color_transform.bg_blend.to_bits(),
                desaturate_bits: cell_color_transform.desaturate.to_bits(),
            },
        }
    }

    fn build_cell_render_info(
        &self,
        col: usize,
        row: usize,
        term_line: i32,
        cell_content: &alacritty_terminal::term::cell::Cell,
        context: PaneCellBuildContext<'_>,
    ) -> CellRenderInfo {
        let resolved_colors = resolve_cell_colors(cell_content, context);
        let text_attributes = cell_text_attributes(cell_content.flags);

        let (search_current, search_match) = if let Some(results) = context.pane_search_results {
            let is_current = results.is_current_match(term_line, col);
            let is_any = results.is_any_match(term_line, col);
            (is_current, is_any && !is_current)
        } else {
            (false, false)
        };

        CellRenderInfo {
            col,
            row,
            char: cell_content.c,
            fg: resolved_colors.fg.into(),
            bg: resolved_colors.bg.into(),
            uses_terminal_default_bg: resolved_colors.uses_terminal_default_bg,
            bold: text_attributes.bold,
            italic: text_attributes.italic,
            underline: text_attributes.underline,
            strikethrough: text_attributes.strikethrough,
            render_text: !cell_content.flags.intersects(
                Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN,
            ),
            selected: selection_range_contains(context.selection_range, col, term_line),
            search_current,
            search_match,
        }
    }

    fn rebuild_pane_render_cache(
        &self,
        terminal: &Terminal,
        cols: usize,
        rows: usize,
        display_offset: usize,
        context: PaneCellBuildContext<'_>,
    ) -> PaneRenderCells {
        if rows == 0 {
            return Arc::new(Vec::new());
        }
        if cols == 0 {
            return Arc::new((0..rows).map(|_| Arc::new(Vec::new())).collect());
        }

        let mut row_cells: Vec<Vec<CellRenderInfo>> =
            (0..rows).map(|_| Vec::with_capacity(cols)).collect();
        let mut expected_row = 0usize;
        let mut expected_col = 0usize;
        let mut ordering_failed = false;

        let _ = terminal.for_each_renderable_cell(
            |cell_display_offset, term_line, col, cell_content| {
                if ordering_failed || cell_display_offset != display_offset {
                    return;
                }
                if col >= cols {
                    ordering_failed = true;
                    return;
                }
                let Some(row) = Self::viewport_row_from_term_line(term_line, cell_display_offset)
                else {
                    ordering_failed = true;
                    return;
                };
                if row >= rows || row != expected_row || col != expected_col {
                    ordering_failed = true;
                    return;
                }

                row_cells[row].push(self.build_cell_render_info(
                    col,
                    row,
                    term_line,
                    cell_content,
                    context,
                ));

                expected_col += 1;
                if expected_col == cols {
                    expected_col = 0;
                    expected_row += 1;
                }
            },
        );

        let fully_populated = expected_row == rows
            && expected_col == 0
            && row_cells.iter().all(|row| row.len() == cols);
        if !ordering_failed && fully_populated {
            return Arc::new(row_cells.into_iter().map(Arc::new).collect());
        }

        self.rebuild_pane_render_cache_fallback(terminal, cols, rows, display_offset, context)
    }

    fn rebuild_pane_render_cache_fallback(
        &self,
        terminal: &Terminal,
        cols: usize,
        rows: usize,
        display_offset: usize,
        context: PaneCellBuildContext<'_>,
    ) -> PaneRenderCells {
        let (default_fg, default_bg) = resolved_default_cell_colors(context);
        let default_cell = CellRenderInfo {
            col: 0,
            row: 0,
            char: ' ',
            fg: default_fg.into(),
            bg: default_bg.into(),
            uses_terminal_default_bg: true,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            render_text: false,
            selected: false,
            search_current: false,
            search_match: false,
        };
        let mut rows_cache: Vec<Vec<CellRenderInfo>> = Vec::with_capacity(rows);
        for row in 0..rows {
            let mut row_cells = vec![default_cell.clone(); cols];
            for (col, cell) in row_cells.iter_mut().enumerate() {
                cell.col = col;
                cell.row = row;
            }
            rows_cache.push(row_cells);
        }

        let _ = terminal.for_each_renderable_cell(
            |cell_display_offset, term_line, col, cell_content| {
                if cell_display_offset != display_offset || col >= cols {
                    return;
                }
                let Some(row) = Self::viewport_row_from_term_line(term_line, cell_display_offset)
                else {
                    return;
                };
                if row >= rows {
                    return;
                }

                rows_cache[row][col] =
                    self.build_cell_render_info(col, row, term_line, cell_content, context);
            },
        );

        Arc::new(rows_cache.into_iter().map(Arc::new).collect())
    }

    #[allow(clippy::too_many_arguments)]
    fn patch_pane_render_cache(
        &self,
        terminal: &Terminal,
        cols: usize,
        rows: usize,
        display_offset: usize,
        cells: &mut PaneRenderCells,
        spans: &[TerminalDirtySpan],
        context: PaneCellBuildContext<'_>,
    ) -> (usize, bool) {
        if !pane_render_cells_match_dimensions(cells, cols, rows) {
            *cells = self.rebuild_pane_render_cache(terminal, cols, rows, display_offset, context);
            return (0, true);
        }

        let mut patched_cell_count = 0usize;
        // Clone the outer row table at most once for this partial update. Each
        // dirty row is then made mutable once before all cells in its span are
        // patched, instead of checking both Arcs for every individual cell.
        let cells: &mut Vec<_> = Arc::make_mut(cells);
        let _ = terminal.with_grid(|grid| {
            let Some(screen_lines) = i32::try_from(grid.screen_lines()).ok() else {
                return;
            };
            let Some(total_lines) = i32::try_from(grid.total_lines()).ok() else {
                return;
            };
            let min_line = -(total_lines - screen_lines);
            let max_line = screen_lines - 1;

            for span in spans {
                if span.row >= rows || cols == 0 {
                    continue;
                }

                let Some(term_line) = term_line_from_viewport_row(span.row, display_offset) else {
                    continue;
                };
                if term_line < min_line || term_line > max_line {
                    continue;
                }

                let row = span.row;
                let line_ref = &grid[Line(term_line)];
                let left_col = span.left_col.min(cols.saturating_sub(1));
                let right_col = span.right_col.min(cols.saturating_sub(1));
                if left_col > right_col {
                    continue;
                }

                patched_cell_count = patched_cell_count.saturating_add(patch_pane_render_row(
                    cells,
                    rows,
                    cols,
                    row,
                    left_col,
                    right_col,
                    |col| {
                        let cell_content = &line_ref[Column(col)];
                        self.build_cell_render_info(col, row, term_line, cell_content, context)
                    },
                ));
            }
        });

        (patched_cell_count, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn update_pane_render_cache(
        &self,
        terminal: &Terminal,
        cols: usize,
        rows: usize,
        display_offset: usize,
        cache: &mut TerminalPaneRenderCache,
        cache_key: TerminalPaneRenderCacheKey,
        context: PaneCellBuildContext<'_>,
        #[cfg(debug_assertions)] render_pass_cache_counts: &mut RenderPassCacheStrategyCounts,
    ) -> (
        PaneRenderCells,
        PaneCacheUpdateStrategy,
        TerminalGridPaintDamage,
    ) {
        let damage = terminal.take_damage_snapshot();
        let mut strategy = pane_cache_update_strategy(
            !cache.cells.is_empty(),
            cache.cols == cols && cache.rows == rows,
            cache.display_offset == display_offset,
            cache.key.as_ref() == Some(&cache_key),
            &damage,
        );
        if strategy == PaneCacheUpdateStrategy::Partial
            && let TerminalDamageSnapshot::Partial(spans) = &damage
            && partial_damage_should_rebuild_full(spans, rows, cols)
        {
            strategy = PaneCacheUpdateStrategy::Full;
        }
        let mut paint_damage = match strategy {
            PaneCacheUpdateStrategy::Reuse => TerminalGridPaintDamage::None,
            PaneCacheUpdateStrategy::Full => TerminalGridPaintDamage::Full,
            PaneCacheUpdateStrategy::Partial => match &damage {
                TerminalDamageSnapshot::Partial(spans) => {
                    paint_damage_from_dirty_spans(spans, rows)
                }
                TerminalDamageSnapshot::Full => TerminalGridPaintDamage::Full,
            },
        };

        match strategy {
            PaneCacheUpdateStrategy::Reuse => {}
            PaneCacheUpdateStrategy::Full => {
                cache.cells =
                    self.rebuild_pane_render_cache(terminal, cols, rows, display_offset, context);
            }
            PaneCacheUpdateStrategy::Partial => {
                let TerminalDamageSnapshot::Partial(spans) = damage else {
                    cache.cells = self.rebuild_pane_render_cache(
                        terminal,
                        cols,
                        rows,
                        display_offset,
                        context,
                    );
                    cache.cols = cols;
                    cache.rows = rows;
                    cache.display_offset = display_offset;
                    cache.key = Some(cache_key);
                    return (
                        cache.cells.clone(),
                        PaneCacheUpdateStrategy::Full,
                        TerminalGridPaintDamage::Full,
                    );
                };
                #[cfg_attr(not(debug_assertions), allow(unused_variables))]
                let (patched_cell_count, did_full_rebuild) = self.patch_pane_render_cache(
                    terminal,
                    cols,
                    rows,
                    display_offset,
                    &mut cache.cells,
                    &spans,
                    context,
                );
                strategy = finalized_cache_update_strategy(strategy, did_full_rebuild);
                if strategy == PaneCacheUpdateStrategy::Full {
                    paint_damage = TerminalGridPaintDamage::Full;
                } else {
                    #[cfg(debug_assertions)]
                    if patched_cell_count > 0 {
                        render_pass_cache_counts
                            .record_partial_work(spans.len(), patched_cell_count);
                    }
                }
            }
        }

        cache.cols = cols;
        cache.rows = rows;
        cache.display_offset = display_offset;
        cache.key = Some(cache_key);
        (cache.cells.clone(), strategy, paint_damage)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_terminal_grid_from_cache(
        &self,
        cells: PaneRenderCells,
        paint_cache: TerminalGridPaintCacheHandle,
        paint_damage: TerminalGridPaintDamage,
        cell_size: Size<Pixels>,
        cols: usize,
        rows: usize,
        colors: &TerminalColors,
        hovered_link_range: Option<(usize, usize, usize, usize)>,
        font_family: SharedString,
        font_size: Pixels,
        cursor_style: TerminalCursorStyle,
        cursor_cell: Option<(usize, usize)>,
        cursor_visible: bool,
        terminal_surface_bg: gpui::Rgba,
    ) -> TerminalGrid {
        let mut selection_bg = colors.cursor;
        selection_bg.a = SELECTION_BG_ALPHA;
        let selection_fg = colors.background;
        TerminalGrid {
            cells,
            paint_cache,
            paint_damage,
            cell_size,
            cols,
            rows,
            // The shared terminal surface already owns the translucent default
            // background. Clearing the grid to that same translucent color would
            // composite it twice and darken the viewport rectangle.
            clear_bg: gpui::Hsla::transparent_black(),
            terminal_surface_bg: terminal_surface_bg.into(),
            cursor_color: colors.cursor.into(),
            selection_bg: selection_bg.into(),
            selection_fg: selection_fg.into(),
            search_match_bg: gpui::Hsla {
                h: 0.14,
                s: 0.92,
                l: 0.62,
                a: 0.62,
            },
            search_current_bg: gpui::Hsla {
                h: 0.09,
                s: 0.98,
                l: 0.56,
                a: 0.86,
            },
            hovered_link_range,
            cursor_cell,
            cursor_visible,
            font_family,
            font_size,
            cursor_style,
        }
    }

    #[cfg(debug_assertions)]
    fn record_render_metrics_for_pass(&mut self, cache_counts: RenderPassCacheStrategyCounts) {
        if !self.render_metrics.enabled {
            return;
        }
        increment_render_count_counter(&mut self.render_metrics.counters);
        self.render_metrics.counters.cache_full_count = self
            .render_metrics
            .counters
            .cache_full_count
            .saturating_add(cache_counts.full);
        self.render_metrics.counters.cache_partial_count = self
            .render_metrics
            .counters
            .cache_partial_count
            .saturating_add(cache_counts.partial);
        self.render_metrics.counters.cache_reuse_count = self
            .render_metrics
            .counters
            .cache_reuse_count
            .saturating_add(cache_counts.reuse);
        self.render_metrics.counters.dirty_span_count = self
            .render_metrics
            .counters
            .dirty_span_count
            .saturating_add(cache_counts.dirty_span_count);
        self.render_metrics.counters.patched_cell_count = self
            .render_metrics
            .counters
            .patched_cell_count
            .saturating_add(cache_counts.patched_cell_count);
    }

    #[cfg(debug_assertions)]
    fn maybe_emit_render_metrics_log(&mut self, now: Instant) {
        if !self.render_metrics.enabled {
            return;
        }

        if let Some(last_emit) = self.render_metrics.last_emit_at
            && now.duration_since(last_emit) < self.render_metrics.log_interval
        {
            return;
        }

        let terminal_ui_snapshot = terminal_ui_render_metrics_snapshot();
        let counters_delta = self
            .render_metrics
            .counters
            .saturating_sub(self.render_metrics.last_emit_counters);
        let terminal_ui_delta =
            terminal_ui_snapshot.saturating_sub(self.render_metrics.last_emit_terminal_ui);
        let dt_ms = self
            .render_metrics
            .last_emit_at
            .map_or(0, |last_emit| now.duration_since(last_emit).as_millis());

        log::info!(
            "render_metrics dt_ms={} render={} grid_paint={} full={} partial={} reuse={} dirty_span={} patched_cell={} shape_line={} shape_hit={} shape_miss={} total_render={} total_grid_paint={} total_full={} total_partial={} total_reuse={} total_dirty_span={} total_patched_cell={} total_shape_line={} total_shape_hit={} total_shape_miss={}",
            dt_ms,
            counters_delta.render_count,
            terminal_ui_delta.grid_paint_count,
            counters_delta.cache_full_count,
            counters_delta.cache_partial_count,
            counters_delta.cache_reuse_count,
            counters_delta.dirty_span_count,
            counters_delta.patched_cell_count,
            terminal_ui_delta.shape_line_calls,
            terminal_ui_delta.shaped_line_cache_hits,
            terminal_ui_delta.shaped_line_cache_misses,
            self.render_metrics.counters.render_count,
            terminal_ui_snapshot.grid_paint_count,
            self.render_metrics.counters.cache_full_count,
            self.render_metrics.counters.cache_partial_count,
            self.render_metrics.counters.cache_reuse_count,
            self.render_metrics.counters.dirty_span_count,
            self.render_metrics.counters.patched_cell_count,
            terminal_ui_snapshot.shape_line_calls,
            terminal_ui_snapshot.shaped_line_cache_hits,
            terminal_ui_snapshot.shaped_line_cache_misses,
        );

        self.render_metrics.last_emit_counters = self.render_metrics.counters;
        self.render_metrics.last_emit_terminal_ui = terminal_ui_snapshot;
        self.render_metrics.last_emit_at = Some(now);
    }

    fn refresh_terminal_scrollbar_marker_cache(
        &mut self,
        layout: terminal_scrollbar::TerminalScrollbarLayout,
        marker_height: f32,
    ) -> Option<f32> {
        if !self.search_open {
            self.clear_terminal_scrollbar_marker_cache();
            return None;
        }

        let marker_height = marker_height.max(0.0);
        let marker_top_limit =
            terminal_scrollbar::marker_top_limit(layout.metrics.track_height, marker_height);
        let cache_key = TerminalScrollbarMarkerCacheKey {
            results_revision: self.search_state.results_revision(),
            history_size: layout.history_size,
            viewport_rows: layout.viewport_rows,
            marker_top_limit_bucket: terminal_scrollbar::marker_top_limit_bucket(marker_top_limit),
        };
        let rebuild_markers = self.terminal_scrollbar_marker_cache.key.as_ref() != Some(&cache_key);

        let (is_empty, current_line, new_marker_tops) = {
            let results = self.search_state.results();
            if results.is_empty() {
                (true, None, None)
            } else {
                let current_line = results.current().map(|current| current.line);
                let new_marker_tops = rebuild_markers.then(|| {
                    terminal_scrollbar::deduped_marker_tops(
                        results
                            .matches()
                            .iter()
                            .map(|search_match| search_match.line),
                        layout.history_size,
                        layout.viewport_rows,
                        marker_height,
                        marker_top_limit,
                    )
                });
                (false, current_line, new_marker_tops)
            }
        };

        if is_empty {
            self.clear_terminal_scrollbar_marker_cache();
            return None;
        }

        if let Some(marker_tops) = new_marker_tops {
            self.terminal_scrollbar_marker_cache.marker_tops = marker_tops;
            self.terminal_scrollbar_marker_cache.key = Some(cache_key);
        }

        current_line.map(|line| {
            terminal_scrollbar::marker_top_for_line(
                line,
                layout.history_size,
                layout.viewport_rows,
                marker_top_limit,
            )
        })
    }

    fn render_terminal_scrollbar_overlay(
        &mut self,
        surface: TerminalScrollbarSurfaceGeometry,
        layout: terminal_scrollbar::TerminalScrollbarLayout,
        force_visible: bool,
    ) -> Option<AnyElement> {
        let now = Instant::now();
        let force_visible = force_visible
            && self.terminal_scrollbar_mode() != ui_scrollbar::ScrollbarVisibilityMode::AlwaysOff;
        let alpha = if force_visible {
            1.0
        } else {
            self.terminal_scrollbar_alpha(now)
        };
        if alpha <= f32::EPSILON && !self.terminal_scrollbar_visibility_controller.is_dragging() {
            return None;
        }
        let overlay_style = self.overlay_style();
        let gutter_bg = overlay_style.panel_background(TERMINAL_SCROLLBAR_GUTTER_ALPHA);
        let frame = terminal_scrollbar_overlay_frame(surface)?;
        let track_width = terminal_scrollbar_track_width(frame.width);
        let style = ScrollbarPaintStyle {
            width: track_width,
            track_radius: TERMINAL_SCROLLBAR_TRACK_RADIUS,
            thumb_radius: TERMINAL_SCROLLBAR_THUMB_RADIUS,
            thumb_inset: TERMINAL_SCROLLBAR_THUMB_INSET,
            marker_inset: TERMINAL_SCROLLBAR_THUMB_INSET,
            marker_radius: TERMINAL_SCROLLBAR_THUMB_RADIUS,
            track_color: self.scrollbar_color(overlay_style, TERMINAL_SCROLLBAR_TRACK_ALPHA),
            thumb_color: self.scrollbar_color(overlay_style, TERMINAL_SCROLLBAR_THUMB_ALPHA),
            active_thumb_color: self
                .scrollbar_color(overlay_style, TERMINAL_SCROLLBAR_THUMB_ACTIVE_ALPHA),
            marker_color: Some(
                self.scrollbar_color(overlay_style, TERMINAL_SCROLLBAR_MATCH_MARKER_ALPHA),
            ),
            current_marker_color: Some(
                overlay_style.panel_cursor(TERMINAL_SCROLLBAR_CURRENT_MARKER_ALPHA),
            ),
        }
        .scale_alpha(alpha);

        let current_marker_top =
            self.refresh_terminal_scrollbar_marker_cache(layout, TERMINAL_SCROLLBAR_MARKER_HEIGHT);
        let marker_tops = &self.terminal_scrollbar_marker_cache.marker_tops;

        Some(
            div()
                .id("terminal-scrollbar-overlay")
                .absolute()
                .left(px(frame.left))
                .top(px(frame.top))
                .w(px(frame.width))
                .h(px(frame.height))
                .bg(gutter_bg)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(px(track_width))
                        .child(ui_scrollbar::render_vertical(
                            "terminal-scrollbar",
                            layout.metrics,
                            style,
                            self.terminal_scrollbar_visibility_controller.is_dragging(),
                            marker_tops,
                            current_marker_top,
                            TERMINAL_SCROLLBAR_MARKER_HEIGHT,
                        )),
                )
                .into_any_element(),
        )
    }

    fn render_update_banner(
        &mut self,
        state: &UpdateState,
        colors: &TerminalColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let model = termy_auto_update_ui::UpdateBannerModel::from_state(state)?;
        let updater_weak = self.auto_updater.as_ref().map(|e| e.downgrade());
        let overlay_style = self.overlay_style();

        let banner_bg = overlay_style.chrome_panel_background_with_floor(0.92, 0.86);
        let border_color = overlay_style.chrome_panel_neutral(0.18);
        let primary_text = overlay_style.panel_foreground(0.94);
        let muted_text = overlay_style.panel_foreground(0.62);

        let (tone_bg, tone_fg) = match model.tone {
            termy_auto_update_ui::UpdateBannerTone::Info => {
                let mut bg = colors.cursor;
                bg.a = 0.18;
                (bg, colors.cursor)
            }
            termy_auto_update_ui::UpdateBannerTone::Success => {
                let success = gpui::Rgba {
                    r: 0.42,
                    g: 0.78,
                    b: 0.55,
                    a: 1.0,
                };
                let mut bg = success;
                bg.a = 0.18;
                (bg, success)
            }
            termy_auto_update_ui::UpdateBannerTone::Error => {
                let error = gpui::Rgba {
                    r: 0.92,
                    g: 0.45,
                    b: 0.48,
                    a: 1.0,
                };
                let mut bg = error;
                bg.a = 0.18;
                (bg, error)
            }
        };

        let icon_path = match model.tone {
            termy_auto_update_ui::UpdateBannerTone::Info => {
                "icons/command_palette/check-update.svg"
            }
            termy_auto_update_ui::UpdateBannerTone::Success => "icons/command_palette/info.svg",
            termy_auto_update_ui::UpdateBannerTone::Error => "icons/command_palette/info.svg",
        };

        let mut actions = div().flex().items_center().gap(px(6.0));
        for button in model.buttons {
            let action = button.action;
            let updater_weak = updater_weak.clone();
            let is_primary = matches!(
                button.style,
                termy_auto_update_ui::UpdateButtonStyle::Primary
            );
            let primary_bg = colors.cursor;
            let primary_label_color = colors.background;
            let mut secondary_bg = colors.foreground;
            secondary_bg.a = 0.08;
            let mut secondary_hover_bg = colors.foreground;
            secondary_hover_bg.a = 0.14;

            let mut primary_hover_bg = colors.cursor;
            primary_hover_bg.a = 0.85;

            let (button_bg, hover_bg, button_text) = if is_primary {
                (primary_bg, primary_hover_bg, primary_label_color)
            } else {
                (secondary_bg, secondary_hover_bg, primary_text)
            };

            actions = actions.child(
                div()
                    .id(gpui::ElementId::from(gpui::SharedString::from(format!(
                        "update-banner-btn-{action:?}"
                    ))))
                    .h(px(26.0))
                    .px(px(12.0))
                    .rounded(px(TERMINAL_OVERLAY_GEOMETRY.control_radius))
                    .bg(button_bg)
                    .text_size(px(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(button_text)
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(move |s| s.bg(hover_bg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| match action {
                            termy_auto_update_ui::UpdateBannerAction::Install => {
                                if let Some(ref weak) = updater_weak
                                    && let Some(entity) = weak.upgrade()
                                {
                                    AutoUpdater::install(entity.downgrade(), cx);
                                    termy_toast::info("Downloading update...");
                                    this.notify_overlay(cx);
                                }
                            }
                            termy_auto_update_ui::UpdateBannerAction::Restart => {
                                match this.restart_application_with_persist() {
                                    Ok(()) => {
                                        this.allow_quit_without_prompt = true;
                                        cx.quit();
                                    }
                                    Err(error) => {
                                        termy_toast::error(format!("Restart failed: {error}"));
                                        this.notify_overlay(cx);
                                    }
                                }
                            }
                            termy_auto_update_ui::UpdateBannerAction::Dismiss => {
                                if let Some(ref weak) = updater_weak
                                    && let Some(entity) = weak.upgrade()
                                {
                                    entity.update(cx, |updater, cx| updater.dismiss(cx));
                                }
                            }
                        }),
                    )
                    .child(button.label),
            );
        }

        let progress_element = model.progress_percent.map(|progress| {
            let mut progress_track = colors.foreground;
            progress_track.a = 0.12;
            let progress_width = 220.0;
            let fill_width = (f32::from(progress) / 100.0) * progress_width;

            div()
                .mt(px(6.0))
                .w(px(progress_width))
                .h(px(3.0))
                .rounded_full()
                .bg(progress_track)
                .child(
                    div()
                        .h_full()
                        .w(px(fill_width.max(0.0)))
                        .rounded_full()
                        .bg(colors.cursor),
                )
                .into_any()
        });

        Some(
            div()
                .id("update-dialog")
                .w_full()
                .max_w(px(380.0))
                .flex_none()
                .bg(banner_bg)
                .border_1()
                .border_color(border_color)
                .rounded(px(8.0))
                .shadow_md()
                .child(
                    div()
                        .w_full()
                        .p(px(12.0))
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap(px(10.0))
                                .min_w(px(0.0))
                                .flex_1()
                                .child(
                                    div()
                                        .w(px(30.0))
                                        .h(px(30.0))
                                        .flex_none()
                                        .rounded(px(TERMINAL_OVERLAY_GEOMETRY.control_radius))
                                        .bg(tone_bg)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            gpui::svg()
                                                .path(gpui::SharedString::from(icon_path))
                                                .size(px(15.0))
                                                .text_color(tone_fg),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .min_w(px(0.0))
                                        .gap(px(1.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .child(
                                                    div()
                                                        .text_size(px(13.0))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .text_color(primary_text)
                                                        .overflow_hidden()
                                                        .child(model.message),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.0))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(tone_fg)
                                                        .child(model.badge),
                                                ),
                                        )
                                        .children(model.detail.map(|detail| {
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(muted_text)
                                                .line_height(px(15.0))
                                                .child(detail)
                                                .into_any()
                                        }))
                                        .children(progress_element),
                                ),
                        )
                        .child(actions),
                )
                .into_any(),
        )
    }

    fn render_toast_overlay(
        &mut self,
        colors: &TerminalColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.toast_manager.active().is_empty() {
            return None;
        }

        let mut container = div().flex().flex_col().gap(px(6.0));
        for toast in self.toast_manager.active() {
            let toast_id = toast.id;
            let toast_message = toast.message.clone();
            let toast_action_label = toast.action_label.clone();
            let is_hovered = self.hovered_toast == Some(toast_id);
            let is_copied = self
                .copied_toast_feedback
                .is_some_and(|(id, _)| id == toast_id);

            // Animation values
            let opacity = toast.opacity();
            let slide_offset = toast.slide_offset();

            // Clean, minimal icons and subtle accent colors
            let (icon, accent, _is_loading) = match toast.kind {
                termy_toast::ToastKind::Info => (
                    "\u{2139}", // ℹ info symbol
                    gpui::Rgba {
                        r: 0.53,
                        g: 0.70,
                        b: 0.92,
                        a: opacity,
                    },
                    false,
                ),
                termy_toast::ToastKind::Success => (
                    "\u{2713}", // ✓ checkmark
                    gpui::Rgba {
                        r: 0.42,
                        g: 0.78,
                        b: 0.55,
                        a: opacity,
                    },
                    false,
                ),
                termy_toast::ToastKind::Warning => (
                    "\u{26A0}", // ⚠ warning
                    gpui::Rgba {
                        r: 0.94,
                        g: 0.76,
                        b: 0.38,
                        a: opacity,
                    },
                    false,
                ),
                termy_toast::ToastKind::Error => (
                    "\u{2715}", // ✕ x mark
                    gpui::Rgba {
                        r: 0.92,
                        g: 0.45,
                        b: 0.45,
                        a: opacity,
                    },
                    false,
                ),
                termy_toast::ToastKind::Loading => {
                    // Animated spinner using braille characters
                    const SPINNER_FRAMES: &[&str] =
                        &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                    let elapsed_ms = toast.created_at.elapsed().as_millis() as usize;
                    let frame_index = (elapsed_ms / 80) % SPINNER_FRAMES.len();
                    (
                        SPINNER_FRAMES[frame_index],
                        gpui::Rgba {
                            r: 0.53,
                            g: 0.70,
                            b: 0.92,
                            a: opacity,
                        },
                        true,
                    )
                }
            };

            // Solid background so toasts remain readable over transparent terminals
            let mut bg = colors.background;
            bg.a = 1.0 * opacity;
            let mut border = colors.foreground;
            border.a = 0.08 * opacity;
            let mut text = colors.foreground;
            text.a = 0.92 * opacity;

            // Icon background with subtle accent tint
            let mut icon_bg = accent;
            icon_bg.a = 0.12 * opacity;

            container = container.child(
                div()
                    .id(("toast", toast_id))
                    .max_w(px(480.0))
                    .mt(px(slide_offset))
                    .rounded(px(TOAST_GEOMETRY.panel_radius))
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .shadow_lg()
                    .child(
                        div()
                            .px(px(14.0))
                            .py(px(12.0))
                            .flex()
                            .items_start()
                            .gap(px(10.0))
                            // Icon with rounded background
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .rounded(px(6.0))
                                    .bg(icon_bg)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(13.0))
                                    .text_color(accent)
                                    .child(icon),
                            )
                            .child(
                                div()
                                    .max_w(px(340.0))
                                    .text_size(px(13.0))
                                    .text_color(text)
                                    .child(toast_message.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(68.0))
                                    .h(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .children(toast_action_label.as_ref().map(|label| {
                                        let label = label.clone();
                                        let mut action_bg = accent;
                                        action_bg.a = 0.18;
                                        div()
                                            .rounded(px(TOAST_GEOMETRY.control_radius))
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .text_size(px(11.0))
                                            .text_color(accent)
                                            .bg(action_bg)
                                            .hover(move |style| {
                                                let mut hover_bg = accent;
                                                hover_bg.a = 0.32;
                                                style.bg(hover_bg)
                                            })
                                            .cursor_pointer()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _event, _window, cx| {
                                                    crate::config::execute_fix_for_toast(toast_id);
                                                    termy_toast::dismiss_toast(toast_id);
                                                    termy_toast::success("Config fixed");
                                                    this.notify_overlay(cx);
                                                    cx.stop_propagation();
                                                }),
                                            )
                                            .child(label)
                                    }))
                                    .children((toast_action_label.is_none() && is_copied).then(|| {
                                        let mut copied_bg = accent;
                                        copied_bg.a = 0.22;
                                        div()
                                            .rounded(px(TOAST_GEOMETRY.control_radius))
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .text_size(px(11.0))
                                            .text_color(accent)
                                            .bg(copied_bg)
                                            .child("Copied")
                                    }))
                                    .children((toast_action_label.is_none() && !is_copied && is_hovered).then(|| {
                                        let toast_message_for_copy = toast_message.clone();
                                        div()
                                            .rounded(px(TOAST_GEOMETRY.control_radius))
                                            .px(px(8.0))
                                            .py(px(4.0))
                                            .text_size(px(11.0))
                                            .text_color(text)
                                            .bg(border)
                                            .hover(|style| style.bg(accent))
                                            .cursor_pointer()
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(
                                                    move |this, _event, _window, cx| {
                                                        cx.write_to_clipboard(
                                                            ClipboardItem::new_string(
                                                                toast_message_for_copy.clone(),
                                                            ),
                                                        );
                                                        this.copied_toast_feedback =
                                                            Some((toast_id, Instant::now()));
                                                        this.notify_overlay(cx);
                                                        cx.spawn(
                                                            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                                                                smol::Timer::after(Duration::from_millis(
                                                                    TOAST_COPY_FEEDBACK_MS,
                                                                ))
                                                                .await;
                                                                let _ = cx.update(|cx| {
                                                                    this.update(cx, |view, cx| {
                                                                        if view
                                                                            .copied_toast_feedback
                                                                            .is_some_and(
                                                                                |(id, _)| {
                                                                                    id == toast_id
                                                                                },
                                                                            )
                                                                        {
                                                                            view.copied_toast_feedback = None;
                                                                            view.notify_overlay(cx);
                                                                        }
                                                                    })
                                                                });
                                                            },
                                                        )
                                                        .detach();
                                                        cx.stop_propagation();
                                                    },
                                                ),
                                            )
                                            .child("Copy")
                                    })),
                            )
                            .on_mouse_move(cx.listener(move |this, _event, _window, cx| {
                                if this.hovered_toast != Some(toast_id) {
                                    this.hovered_toast = Some(toast_id);
                                    this.notify_overlay(cx);
                                }
                                cx.stop_propagation();
                            })),
                    ),
            );
        }

        Some(
            div()
                .id("toast-overlay")
                .size_full()
                .absolute()
                .top_0()
                .left_0()
                .child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_end()
                        .justify_end()
                        .pr(px(20.0))
                        .pb(px(20.0))
                        .on_mouse_move(cx.listener(|this, _event, _window, cx| {
                            if this.hovered_toast.is_some() {
                                this.hovered_toast = None;
                                this.notify_overlay(cx);
                            }
                        }))
                        .child(container),
                )
                .into_any(),
        )
    }

    fn render_link_preview_overlay(&self) -> Option<AnyElement> {
        let link = self.hovered_link.as_ref()?;
        let overlay_style = self.overlay_style();

        let url = &link.target;
        let display_url = if let Some(path) = url.strip_prefix("file:///") {
            let path = format!("/{path}");
            let path = path.replace("%20", " ");
            #[cfg(unix)]
            let path = if let Some(home) = dirs::home_dir() {
                let home_str = home.to_string_lossy();
                if let Some(rel) = path.strip_prefix(home_str.as_ref()) {
                    format!("~{rel}")
                } else {
                    path
                }
            } else {
                path
            };
            if path.len() > 80 {
                format!("…{}", &path[path.len() - 79..])
            } else {
                path
            }
        } else if url.len() > 80 {
            format!("{}…", &url[..79])
        } else {
            url.clone()
        };

        Some(
            div()
                .id("link-preview-overlay")
                .absolute()
                .bottom(px(8.0))
                .left(px(8.0))
                .max_w(px(600.0))
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(TERMINAL_OVERLAY_GEOMETRY.panel_radius))
                .bg(overlay_style.chrome_panel_background(0.92))
                .border_1()
                .border_color(overlay_style.chrome_panel_neutral(0.22))
                .shadow_sm()
                .text_size(px(11.5))
                .text_color(overlay_style.panel_foreground(0.90))
                .child(display_url)
                .into_any_element(),
        )
    }

    fn schedule_progress_indicator_animation(&mut self, cx: &mut Context<Self>) {
        if self.progress_indicator_animation_scheduled {
            return;
        }

        self.progress_indicator_animation_scheduled = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(16)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.progress_indicator_animation_scheduled = false;
                    if view.active_tab_has_indeterminate_progress() {
                        cx.notify();
                    }
                })
            });
        })
        .detach();
    }

    fn active_tab_has_indeterminate_progress(&self) -> bool {
        self.tabs.get(self.active_tab).is_some_and(|tab| {
            tab.panes
                .iter()
                .any(|pane| pane.progress_state.is_indeterminate())
        })
    }

    /// Thin progress bar pinned to the top edge of a single pane, driven by
    /// that pane's own OSC 9;4 state.
    fn pane_progress_loader_element(&self, state: ProgressState) -> Option<AnyElement> {
        if !self.progress_indicator_enabled || !state.is_active() {
            return None;
        }

        let elapsed_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let fill = terminal_progress_loader_fill(state, elapsed_ms)?;
        let mut track_color = self.colors.foreground;
        track_color.a = self.scaled_chrome_alpha(0.10);
        let mut fill_color = match state {
            ProgressState::InProgress(_) | ProgressState::Indeterminate => self.colors.cursor,
            ProgressState::Error(_) => gpui::rgb(0xef4444),
            ProgressState::Warning(_) => gpui::rgb(0xf59e0b),
            ProgressState::Clear => return None,
        };
        fill_color.a = self.scaled_chrome_alpha(0.92);

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(TERMINAL_PROGRESS_LOADER_HEIGHT))
                .overflow_hidden()
                .rounded_full()
                .bg(track_color)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(relative(fill.left_ratio))
                        .w(relative(fill.width_ratio))
                        .rounded_full()
                        .bg(fill_color),
                )
                .into_any_element(),
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn clamped_context_menu_origin(
        &self,
        anchor: gpui::Point<Pixels>,
        menu_width: f32,
        menu_height: f32,
    ) -> (f32, f32) {
        let mut x: f32 = anchor.x.into();
        let mut y: f32 = anchor.y.into();

        if let Some((viewport_width, viewport_height)) = self.last_viewport_size_px {
            let max_x = (viewport_width as f32 - menu_width).max(0.0);
            let max_y = (viewport_height as f32 - menu_height).max(0.0);
            x = x.clamp(0.0, max_x);
            y = y.clamp(0.0, max_y);
        }

        (x, y)
    }

    fn render_terminal_context_menu_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        #[cfg(target_os = "macos")]
        {
            let _ = cx;
            None
        }

        #[cfg(not(target_os = "macos"))]
        {
            let state = self.terminal_context_menu.clone()?;
            let plugin_commands = self.plugin_commands_for_placement(
                termy_plugin_runtime::PluginCommandPlacement::TerminalContextMenu,
            );
            let overlay_style = self.overlay_style();
            let menu_width = if plugin_commands.is_empty() {
                220.0
            } else {
                260.0
            };
            let row_height = 30.0;
            let row_count =
                3.0 + if state.buffer_position.is_some() {
                    1.0
                } else {
                    0.0
                } + if state.image.is_some() { 1.0 } else { 0.0 }
                    + plugin_commands.len() as f32;
            let content_height =
                row_height * row_count + 8.0 + if plugin_commands.is_empty() { 0.0 } else { 7.0 };
            let menu_height = context_menu_visible_height(
                content_height,
                self.last_viewport_size_px.map(|(_, height)| height as f32),
                row_height,
            );
            let (menu_x, menu_y) =
                self.clamped_context_menu_origin(state.anchor_position, menu_width, menu_height);
            let panel_bg = overlay_style.chrome_panel_background(0.98);
            let panel_border = overlay_style.chrome_panel_neutral(0.22);
            let text_active = overlay_style.panel_foreground(0.95);
            let text_disabled = overlay_style.panel_foreground(0.42);
            let hover_bg = overlay_style.chrome_panel_cursor(0.22);
            let buffer_position_item = |label: String| {
                div()
                    .id("terminal-context-menu-buffer-position")
                    .h(px(row_height))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .text_size(px(12.0))
                    .text_color(text_disabled)
                    .child(label)
                    .into_any_element()
            };

            let command_item =
                |id: &'static str, label: &'static str, enabled: bool, action: CommandAction| {
                    let text_color = if enabled { text_active } else { text_disabled };
                    div()
                        .id(id)
                        .h(px(row_height))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .text_size(px(13.0))
                        .text_color(text_color)
                        .when(enabled, |s| s.cursor_pointer())
                        .when(enabled, |s| s.hover(|style| style.bg(hover_bg)))
                        .when(enabled, |s| {
                            s.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, _event: &MouseDownEvent, _window, cx| {
                                    view.execute_terminal_context_menu_command(action, cx);
                                    cx.stop_propagation();
                                }),
                            )
                        })
                        .child(label)
                        .into_any_element()
                };
            let open_search_item = || {
                div()
                    .id("terminal-context-menu-open-search")
                    .h(px(row_height))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.0))
                    .text_color(text_active)
                    .cursor_pointer()
                    .hover(|style| style.bg(hover_bg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _event: &MouseDownEvent, _window, cx| {
                            let _ = view.close_terminal_context_menu(cx);
                            view.open_search(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child("Open Search")
                    .into_any_element()
            };
            let copy_image_item = || {
                div()
                    .id("terminal-context-menu-copy-image")
                    .h(px(row_height))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.0))
                    .text_color(text_active)
                    .cursor_pointer()
                    .hover(|style| style.bg(hover_bg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _event: &MouseDownEvent, _window, cx| {
                            view.execute_terminal_context_menu_copy_image(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child("Copy Image")
                    .into_any_element()
            };
            let plugin_command_item = |command: termy_plugin_runtime::PluginCommand| {
                let enabled = command.disabled_reason.is_none();
                let text_color = if enabled { text_active } else { text_disabled };
                let plugin_id = command.plugin_id.clone();
                let command_id = command.id.clone();
                div()
                    .id(SharedString::from(format!(
                        "terminal-context-menu-plugin-{plugin_id}-{command_id}"
                    )))
                    .h(px(row_height))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.0))
                    .text_color(text_color)
                    .when(enabled, |style| style.cursor_pointer())
                    .when(enabled, |style| style.hover(|style| style.bg(hover_bg)))
                    .when(enabled, |style| {
                        style.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _event: &MouseDownEvent, window, cx| {
                                let _ = view.close_terminal_context_menu(cx);
                                view.start_plugin_command(&plugin_id, &command_id, window, cx);
                                cx.stop_propagation();
                            }),
                        )
                    })
                    .child(command.title)
                    .into_any_element()
            };

            Some(
                div()
                    .id("terminal-context-menu-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _event: &MouseDownEvent, _window, cx| {
                            let _ = view.close_terminal_context_menu(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|view, _event: &MouseDownEvent, _window, cx| {
                            let _ = view.close_terminal_context_menu(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|view, event: &MouseDownEvent, _window, cx| {
                            view.open_terminal_context_menu(event.position, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .id("terminal-context-menu-panel")
                            .absolute()
                            .left(px(menu_x))
                            .top(px(menu_y))
                            .w(px(menu_width))
                            .max_h(px(menu_height))
                            .overflow_y_scroll()
                            .py(px(4.0))
                            .bg(panel_bg)
                            .border_1()
                            .border_color(panel_border)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_view, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(|_view, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|_view, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .when_some(state.buffer_position, |panel, position| {
                                panel.child(buffer_position_item(
                                    TerminalView::format_terminal_buffer_position(position),
                                ))
                            })
                            .child(command_item(
                                "terminal-context-menu-copy",
                                "Copy",
                                state.can_copy,
                                CommandAction::Copy,
                            ))
                            .when(state.image.is_some(), |panel| {
                                panel.child(copy_image_item())
                            })
                            .child(command_item(
                                "terminal-context-menu-paste",
                                "Paste",
                                state.can_paste,
                                CommandAction::Paste,
                            ))
                            .child(open_search_item())
                            .when(!plugin_commands.is_empty(), |panel| {
                                panel.child(
                                    div().h(px(1.0)).my(px(3.0)).mx(px(8.0)).bg(panel_border),
                                )
                            })
                            .children(plugin_commands.into_iter().map(plugin_command_item)),
                    )
                    .into_any_element(),
            )
        }
    }

    /// Dropdown under the tab strip's "+" button offering available tab and
    /// shell choices for the current platform.
    fn render_new_tab_menu_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (menu_x, menu_y) = self.new_tab_menu_anchor?;
        let overlay_style = self.overlay_style();
        let row_height = 30.0;
        let panel_bg = overlay_style.chrome_panel_background(0.98);
        let panel_border = overlay_style.chrome_panel_neutral(0.22);
        let text_color = overlay_style.panel_foreground(0.95);
        let hover_bg = overlay_style.chrome_panel_cursor(0.22);

        let menu_row = |id: &'static str,
                        label: &'static str,
                        cx: &mut Context<Self>,
                        on_select: fn(&mut Self, &mut Context<Self>)| {
            div()
                .id(id)
                .h(px(row_height))
                .px(px(10.0))
                .flex()
                .items_center()
                .text_size(px(13.0))
                .text_color(text_color)
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _event: &MouseDownEvent, _window, cx| {
                        let _ = view.close_new_tab_menu(cx);
                        on_select(view, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(label)
        };

        let default_terminal_label = if cfg!(target_os = "windows") {
            "Default Shell"
        } else {
            "New Terminal Tab"
        };

        let mut panel = div()
            .id("new-tab-menu-panel")
            .absolute()
            .left(px(menu_x))
            .top(px(menu_y))
            .w(px(NEW_TAB_MENU_WIDTH))
            .py(px(4.0))
            .bg(panel_bg)
            .border_1()
            .border_color(panel_border)
            .rounded(px(8.0))
            .shadow_lg()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_view, _event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(menu_row(
                "new-tab-menu-terminal",
                default_terminal_label,
                cx,
                |view, cx| view.add_tab(cx),
            ));

        if cfg!(target_os = "windows") {
            panel = panel
                .child(menu_row(
                    "new-tab-menu-command-prompt",
                    "Command Prompt",
                    cx,
                    |view, cx| {
                        view.add_tab_with_windows_shell(RuntimeWindowsShell::Cmd, cx);
                    },
                ))
                .child(menu_row(
                    "new-tab-menu-windows-powershell",
                    "Windows PowerShell",
                    cx,
                    |view, cx| {
                        view.add_tab_with_windows_shell(RuntimeWindowsShell::PowerShell, cx);
                    },
                ))
                .child(menu_row(
                    "new-tab-menu-powershell-core",
                    "PowerShell 7",
                    cx,
                    |view, cx| {
                        view.add_tab_with_windows_shell(RuntimeWindowsShell::PowerShellCore, cx);
                    },
                ))
                .child(menu_row(
                    "new-tab-menu-git-bash",
                    "Git Bash",
                    cx,
                    |view, cx| {
                        view.add_tab_with_windows_shell(RuntimeWindowsShell::GitBash, cx);
                    },
                ));
        }

        if self.browser_tabs_available() {
            panel = panel.child(menu_row(
                "new-tab-menu-browser",
                "New Browser Tab",
                cx,
                |view, cx| view.add_browser_tab(cx),
            ));
        }

        if !self.saved_ssh_hosts.is_empty() {
            panel = panel
                .child(div().mx(px(8.0)).my(px(4.0)).h(px(1.0)).bg(panel_border))
                .child(
                    div()
                        .h(px(22.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(overlay_style.panel_foreground(0.58))
                        .child("SSH HOSTS"),
                );
            for host in self.saved_ssh_hosts.clone() {
                let row_id = SharedString::from(format!("new-tab-menu-ssh-{}", host.id));
                let host_id = host.id;
                panel = panel.child(
                    div()
                        .id(row_id)
                        .h(px(row_height))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .text_size(px(13.0))
                        .text_color(text_color)
                        .cursor_pointer()
                        .overflow_hidden()
                        .hover(move |style| style.bg(hover_bg))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _event: &MouseDownEvent, _window, cx| {
                                let _ = view.close_new_tab_menu(cx);
                                view.add_ssh_tab(&host_id, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(host.display_name),
                );
            }
        }

        Some(
            div()
                .id("new-tab-menu-overlay")
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|view, _event: &MouseDownEvent, _window, cx| {
                        let _ = view.close_new_tab_menu(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    fn render_tab_context_menu_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        #[cfg(target_os = "macos")]
        {
            let _ = cx;
            None
        }

        #[cfg(not(target_os = "macos"))]
        {
            let state = self.tab_context_menu.clone()?;
            let plugin_commands = self.plugin_commands_for_placement(
                termy_plugin_runtime::PluginCommandPlacement::TabContextMenu,
            );
            let overlay_style = self.overlay_style();
            let menu_width = if plugin_commands.is_empty() {
                180.0
            } else {
                260.0
            };
            let row_height = 30.0;
            let content_height = (row_height * (3 + plugin_commands.len()) as f32)
                + 8.0
                + if plugin_commands.is_empty() { 0.0 } else { 7.0 };
            let menu_height = context_menu_visible_height(
                content_height,
                self.last_viewport_size_px.map(|(_, height)| height as f32),
                row_height,
            );
            let (menu_x, menu_y) =
                self.clamped_context_menu_origin(state.anchor_position, menu_width, menu_height);
            let panel_bg = overlay_style.chrome_panel_background(0.98);
            let panel_border = overlay_style.chrome_panel_neutral(0.22);
            let text_active = overlay_style.panel_foreground(0.95);
            let text_disabled = overlay_style.panel_foreground(0.42);
            let text_danger = gpui::Rgba {
                r: 0.95,
                g: 0.4,
                b: 0.4,
                a: 1.0,
            };
            let hover_bg = overlay_style.chrome_panel_cursor(0.22);
            let pin_label = if state.pinned { "Unpin Tab" } else { "Pin Tab" };
            let plugin_command_item = |command: termy_plugin_runtime::PluginCommand| {
                let enabled = command.disabled_reason.is_none();
                let text_color = if enabled { text_active } else { text_disabled };
                let plugin_id = command.plugin_id.clone();
                let command_id = command.id.clone();
                let tab_id = state.tab_id;
                div()
                    .id(SharedString::from(format!(
                        "tab-context-menu-plugin-{plugin_id}-{command_id}"
                    )))
                    .h(px(row_height))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.0))
                    .text_color(text_color)
                    .when(enabled, |style| style.cursor_pointer())
                    .when(enabled, |style| style.hover(|style| style.bg(hover_bg)))
                    .when(enabled, |style| {
                        style.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _event: &MouseDownEvent, window, cx| {
                                if let Some(index) = view.tab_index_by_id(tab_id) {
                                    view.switch_tab(index, cx);
                                }
                                let _ = view.close_tab_context_menu(cx);
                                view.start_plugin_command(&plugin_id, &command_id, window, cx);
                                cx.stop_propagation();
                            }),
                        )
                    })
                    .child(command.title)
                    .into_any_element()
            };

            Some(
                div()
                    .id("tab-context-menu-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _event: &MouseDownEvent, _window, cx| {
                            let _ = view.close_tab_context_menu(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|view, _event: &MouseDownEvent, _window, cx| {
                            let _ = view.close_tab_context_menu(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|view, _event: &MouseDownEvent, _window, cx| {
                            let _ = view.close_tab_context_menu(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        div()
                            .id("tab-context-menu-panel")
                            .absolute()
                            .left(px(menu_x))
                            .top(px(menu_y))
                            .w(px(menu_width))
                            .max_h(px(menu_height))
                            .overflow_y_scroll()
                            .py(px(4.0))
                            .bg(panel_bg)
                            .border_1()
                            .border_color(panel_border)
                            .rounded(px(8.0))
                            .shadow_lg()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_view, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(|_view, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|_view, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                }),
                            )
                            .children(plugin_commands.iter().cloned().map(plugin_command_item))
                            .when(!plugin_commands.is_empty(), |panel| {
                                panel.child(
                                    div().h(px(1.0)).my(px(3.0)).mx(px(8.0)).bg(panel_border),
                                )
                            })
                            // Rename Tab
                            .child({
                                let tab_id = state.tab_id;
                                div()
                                    .id("tab-context-menu-rename")
                                    .h(px(row_height))
                                    .px(px(10.0))
                                    .flex()
                                    .items_center()
                                    .text_size(px(13.0))
                                    .text_color(text_active)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(hover_bg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |view, _event: &MouseDownEvent, _window, cx| {
                                                if let Some(index) = view.tab_index_by_id(tab_id) {
                                                    view.begin_rename_tab(index, cx);
                                                }
                                                let _ = view.close_tab_context_menu(cx);
                                                cx.stop_propagation();
                                            },
                                        ),
                                    )
                                    .child("Rename Tab")
                            })
                            // Pin/Unpin Tab
                            .child(
                                div()
                                    .id("tab-context-menu-toggle-pin")
                                    .h(px(row_height))
                                    .px(px(10.0))
                                    .flex()
                                    .items_center()
                                    .text_size(px(13.0))
                                    .text_color(text_active)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(hover_bg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |view, _event: &MouseDownEvent, _window, cx| {
                                                let _ = view.set_tab_pinned_by_id(
                                                    state.tab_id,
                                                    !state.pinned,
                                                    cx,
                                                );
                                                let _ = view.close_tab_context_menu(cx);
                                                cx.stop_propagation();
                                            },
                                        ),
                                    )
                                    .child(pin_label),
                            )
                            // Close Tab
                            .child({
                                let tab_id = state.tab_id;
                                div()
                                    .id("tab-context-menu-close")
                                    .h(px(row_height))
                                    .px(px(10.0))
                                    .flex()
                                    .items_center()
                                    .text_size(px(13.0))
                                    .text_color(text_danger)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(hover_bg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |view, _event: &MouseDownEvent, _window, cx| {
                                                if let Some(index) = view.tab_index_by_id(tab_id) {
                                                    view.close_tab(index, cx);
                                                }
                                                let _ = view.close_tab_context_menu(cx);
                                                cx.stop_propagation();
                                            },
                                        ),
                                    )
                                    .child("Close Tab")
                            }),
                    )
                    .into_any_element(),
            )
        }
    }

    pub(super) fn render_overlay_layer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Instant::now();
        self.toast_manager.ingest_pending();
        self.toast_manager.tick_with_hovered(self.hovered_toast);
        if let Some((_, copied_at)) = self.copied_toast_feedback
            && copied_at.elapsed() >= Duration::from_millis(TOAST_COPY_FEEDBACK_MS)
        {
            self.copied_toast_feedback = None;
        }
        if let Some(until) = self.resize_indicator_visible_until
            && now >= until
        {
            self.resize_indicator_visible_until = None;
        }

        // Request re-render during toast animations for smooth fade in/out.
        if self.toast_manager.is_animating() && !self.toast_animation_scheduled {
            self.toast_animation_scheduled = true;
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                smol::Timer::after(Duration::from_millis(16)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.toast_animation_scheduled = false;
                        view.notify_overlay(cx);
                    })
                });
            })
            .detach();
        }
        if let Some(until) = self.resize_indicator_visible_until
            && !self.resize_indicator_animation_scheduled
        {
            let delay = until
                .saturating_duration_since(now)
                .max(Duration::from_millis(1));
            self.resize_indicator_animation_scheduled = true;
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                smol::Timer::after(delay).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.resize_indicator_animation_scheduled = false;
                        view.notify_overlay(cx);
                    })
                });
            })
            .detach();
        }
        let colors = self.colors.clone();
        let command_palette_overlay = if self.is_command_palette_open() {
            Some(self.render_command_palette_modal(window, cx))
        } else {
            None
        };
        let plugin_ui_overlay = self.modal_plugin_ui(cx);
        let search_overlay = if self.search_open {
            Some(self.render_search_bar(cx))
        } else {
            None
        };
        let chrome_height = self.terminal_content_top_inset();
        let terminal_overlay = (command_palette_overlay.is_some()
            || plugin_ui_overlay.is_some()
            || search_overlay.is_some())
        .then(|| {
            div()
                .id("terminal-scoped-overlay")
                .absolute()
                .top(px(chrome_height))
                .left_0()
                .right_0()
                .bottom_0()
                .children(command_palette_overlay)
                .children(plugin_ui_overlay)
                .children(search_overlay)
                .into_any_element()
        });
        let context_menu_overlay = self.render_terminal_context_menu_overlay(cx);
        let tab_context_menu_overlay = self.render_tab_context_menu_overlay(cx);
        let new_tab_menu_overlay = self.render_new_tab_menu_overlay(cx);
        let toast_overlay = self.render_toast_overlay(&colors, cx);
        let link_preview_overlay = self.render_link_preview_overlay();
        let resize_overlay = self
            .resize_indicator_visible_until
            .zip(self.resize_indicator_dims)
            .map(|(_, (cols, rows))| {
                let overlay_style = self.overlay_style();
                div()
                    .id("window-resize-indicator-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(5.0))
                            .rounded_full()
                            .bg(overlay_style.chrome_panel_background(0.54))
                            .border_1()
                            .border_color(overlay_style.chrome_panel_neutral(0.10))
                            .shadow_md()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(overlay_style.panel_foreground(0.82))
                            .child(format!("{cols} x {rows}")),
                    )
                    .into_any_element()
            });
        let debug_overlay = self.show_debug_overlay.then(|| {
            let overlay_style = self.overlay_style();
            let cpu_percent = self.debug_overlay_stats.cpu_percent;
            let render_callbacks_per_second =
                self.debug_overlay_stats.render_callbacks_per_second;
            let memory = self.debug_overlay_memory_label();
            let callback_interval_p50_ms = self
                .debug_overlay_stats
                .render_callback_interval_p50_ms;
            let callback_interval_p95_ms = self
                .debug_overlay_stats
                .render_callback_interval_p95_ms;
            let callback_interval_p99_ms = self
                .debug_overlay_stats
                .render_callback_interval_p99_ms;
            let view_build_p50_ms = self.debug_overlay_stats.view_build_p50_ms;
            let view_build_p95_ms = self.debug_overlay_stats.view_build_p95_ms;
            let view_build_p99_ms = self.debug_overlay_stats.view_build_p99_ms;
            let terminal_event_drain_passes = self.debug_overlay_stats.terminal_event_drain_passes;
            let terminal_redraws = self.debug_overlay_stats.terminal_redraws;
            let alt_screen_fallback_redraws = self.debug_overlay_stats.alt_screen_fallback_redraws;
            let span_damage_ms = self.debug_overlay_stats.span_damage_ms;
            let span_rebuild_ms = self.debug_overlay_stats.span_rebuild_ms;
            let span_shaping_ms = self.debug_overlay_stats.span_shaping_ms;
            let span_paint_ms = self.debug_overlay_stats.span_paint_ms;
            #[cfg(debug_assertions)]
            let view_wake_signals = self.debug_overlay_stats.view_wake_signals;
            #[cfg(debug_assertions)]
            let runtime_wakeups = self.debug_overlay_stats.runtime_wakeups;
            #[cfg(target_os = "macos")]
            let display_hint = "up to 120Hz";
            #[cfg(not(target_os = "macos"))]
            let display_hint = "system";

            let overlay = div()
                .id("debug-metrics-overlay")
                .absolute()
                .top(px(chrome_height + 10.0))
                .right(px(10.0))
                .px(px(10.0))
                .py(px(8.0))
                .rounded(px(TERMINAL_OVERLAY_GEOMETRY.panel_radius))
                .bg(overlay_style.chrome_panel_background(0.84))
                .border_1()
                .border_color(overlay_style.chrome_panel_neutral(0.24))
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(overlay_style.panel_foreground(0.95))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(format!("Display: {display_hint}"))
                .child(format!(
                    "Render callbacks: {render_callbacks_per_second:.1}/s"
                ))
                .child(format!(
                    "Callback interval ms p50/p95/p99: {callback_interval_p50_ms:.2}/{callback_interval_p95_ms:.2}/{callback_interval_p99_ms:.2}"
                ))
                .child(format!(
                    "CPU view build ms p50/p95/p99: {view_build_p50_ms:.2}/{view_build_p95_ms:.2}/{view_build_p99_ms:.2}"
                ))
                .child(format!("CPU: {cpu_percent:.1}%"))
                .child(format!("Process RSS: {memory}"))
                .child(format!("Drain passes: {terminal_event_drain_passes}"))
                .child(format!("Redraws: {terminal_redraws}"))
                .child(format!(
                    "Alt fallback redraws: {alt_screen_fallback_redraws}"
                ))
                .child(format!(
                    "Spans ms: dmg={span_damage_ms:.2} rebuild={span_rebuild_ms:.2} shape={span_shaping_ms:.2} paint={span_paint_ms:.2}"
                ));
            #[cfg(debug_assertions)]
            let overlay = overlay.child(format!(
                "Wakeups runtime/view: {runtime_wakeups}/{view_wake_signals}"
            ));
            overlay.into_any_element()
        });

        let banner_overlay: Option<AnyElement> = if self.update_banner_visible() {
            let banner_state = self.auto_updater.as_ref().map(|e| e.read(cx).state.clone());
            let banner_layout = self.update_banner_layout();
            banner_state
                .as_ref()
                .and_then(|state| self.render_update_banner(state, &colors, cx))
                .map(|banner| {
                    let layout = banner_layout.unwrap_or(UpdateBannerLayout {
                        overlay_top: 0.0,
                        overlay_left: 0.0,
                    });
                    div()
                        .id("update-dialog-overlay")
                        .absolute()
                        .top(px(layout.overlay_top))
                        .left(px(layout.overlay_left))
                        .right_0()
                        .px(px(10.0))
                        .flex()
                        .justify_end()
                        .child(banner)
                        .into_any_element()
                })
        } else {
            None
        };

        div()
            .id("terminal-overlay-layer")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(banner_overlay)
            .children(terminal_overlay)
            .children(context_menu_overlay)
            .children(tab_context_menu_overlay)
            .children(new_tab_menu_overlay)
            .children(resize_overlay)
            .children(debug_overlay)
            .children(toast_overlay)
            .children(link_preview_overlay)
            .into_any_element()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame_now = Instant::now();
        self.record_debug_overlay_frame();
        let view_build_started_at =
            (self.show_debug_overlay || self.inspector_collects_render_stats()).then(Instant::now);
        self.record_benchmark_frame(frame_now);

        // Process pending OSC 52 clipboard writes
        if let Some(text) = self.pending_clipboard.take() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }

        let layout_cell_size = self.calculate_cell_size(window, cx);
        let colors = self.colors.clone();
        let font_family = self.font_family.clone();
        let ui_font_family = self.ui_font_family.clone();
        let font_size = self.font_size;
        self.sync_window_background_appearance(window);
        let effective_background_opacity = self.background_opacity_factor();
        let mut terminal_surface_bg = colors.background;
        terminal_surface_bg.a = self.scaled_background_alpha(terminal_surface_bg.a);

        self.sync_terminal_size(window, layout_cell_size, cx);
        let active_pane_id = self.active_pane_id().map(ToOwned::to_owned);
        let now = frame_now;
        let viewport = window.viewport_size();
        self.last_viewport_width = viewport.width.into();
        self.track_window_resize_indicator(viewport, now);
        let pane_focus_config = self.pane_focus_config();
        let command_palette_open = self.is_command_palette_open();
        let plugin_ui_open = self.plugin_ui.is_some();
        let palette_backdrop_transform =
            (command_palette_open || plugin_ui_open).then(command_palette_backdrop_transform);
        let terminal_cursor_active = !command_palette_open
            && !plugin_ui_open
            && self.renaming_tab.is_none()
            && self.renaming_workspace.is_none()
            && !self.search_open;
        let cursor_visible = terminal_cursor_active
            && self.cursor_visible_for_focus(self.focus_handle.is_focused(window));

        // Pre-compute search match info for active pane.
        let search_active = self.search_open;
        let configured_cursor_style = self.terminal_cursor_style();
        let mut terminal_display_offset = 0usize;
        let divider_rgba = pane_divider_color(terminal_surface_bg, colors.foreground);
        let mut divider_line_rgba = divider_rgba;
        divider_line_rgba.a = self.scaled_chrome_neutral_border_alpha(0.42);
        let divider_line_color: gpui::Hsla = divider_line_rgba.into();
        let mut pane_layers = Vec::<AnyElement>::new();
        let mut pane_dividers = Vec::<AnyElement>::new();
        let mut pane_resize_handles = Vec::<AnyElement>::new();
        let mut pane_focus_accents = Vec::<AnyElement>::new();
        let mut pane_drag_handles = Vec::<AnyElement>::new();
        let mut pane_drop_overlays = Vec::<AnyElement>::new();
        #[cfg(debug_assertions)]
        let mut render_pass_cache_counts = RenderPassCacheStrategyCounts::default();

        let active_pane_font_sizes = self
            .tabs
            .get(self.active_tab)
            .map(|tab| {
                tab.panes
                    .iter()
                    .map(|pane| self.effective_font_size_for_pane_in_tab(tab, pane))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for pane_font_size in active_pane_font_sizes.iter().copied() {
            let _ = self.calculate_cell_size_for_font_size(pane_font_size, window, cx);
        }

        if self.progress_indicator_enabled && self.active_tab_has_indeterminate_progress() {
            self.schedule_progress_indicator_animation(cx);
        }

        if let Some(active_tab) = self.tabs.get(self.active_tab)
            && let Some(content_bounds) = self.terminal_content_bounds(window)
        {
            let multi_pane = active_tab.panes.len() > 1;
            let pane_focus_enabled = multi_pane
                && pane_focus_config.is_some()
                && !command_palette_open
                && !plugin_ui_open;
            let pane_divider_layouts = if multi_pane {
                self.native_pane_dividers(active_tab)
            } else {
                Vec::new()
            };

            for (pane, pane_font_size) in active_tab
                .panes
                .iter()
                .zip(active_pane_font_sizes.iter().copied())
            {
                if pane.is_browser() {
                    let Some(pane_layout) =
                        self.terminal_pane_layout(active_tab, pane, content_bounds)
                    else {
                        continue;
                    };
                    let pane_left = pane_layout.content_frame.origin_x;
                    let pane_top = pane_layout.content_frame.origin_y;
                    let pane_width = pane_layout.content_frame.width;
                    let pane_height = pane_layout.content_frame.height;
                    let pane_id = pane.id.clone();
                    let chrome = self.render_browser_chrome(pane_id, &colors, &ui_font_family, cx);
                    let fallback = pane.browser_state().and_then(|state| {
                        state.webview_creation_error().map(|error| {
                            self.render_browser_fallback(
                                pane.id.clone(),
                                error,
                                &colors,
                                &ui_font_family,
                                cx,
                            )
                        })
                    });
                    pane_layers.push(
                        div()
                            .id(pane.cached_element_ids.pane.clone())
                            .absolute()
                            .left(px(pane_left))
                            .top(px(pane_top))
                            .w(px(pane_width))
                            .h(px(pane_height))
                            .cursor_text()
                            .bg(terminal_surface_bg)
                            .child(chrome)
                            .children(fallback)
                            .into_any_element(),
                    );
                    continue;
                }

                let terminal = pane.terminal();
                let terminal_size = terminal.size();
                let cols = terminal_size.cols as usize;
                let rows = terminal_size.rows as usize;
                if cols == 0 || rows == 0 {
                    continue;
                }
                let is_active_pane = active_pane_id.as_deref() == Some(pane.id.as_str());
                let (pane_inactive_focus, pane_active_focus) =
                    pane_focus_factors(is_active_pane, pane_focus_enabled);
                let (pane_focus_transform, raw_pane_active_border_alpha) =
                    if let Some((preset, strength)) = pane_focus_config {
                        let inactive_scale = strength * pane_inactive_focus;
                        let active_scale = strength * pane_active_focus;
                        (
                            CellColorTransform {
                                fg_blend: preset.inactive_fg_blend * inactive_scale,
                                bg_blend: preset.inactive_bg_blend * inactive_scale,
                                desaturate: preset.inactive_desaturate * inactive_scale,
                            },
                            preset.active_border_alpha * active_scale,
                        )
                    } else {
                        (CellColorTransform::default(), 0.18)
                    };
                // Palette backdrop uses the same inactive-pane transform path to keep one
                // consistent dimming model and avoid a separate full-screen color overlay.
                let cell_color_transform =
                    palette_backdrop_transform.unwrap_or(pane_focus_transform);
                // tmux mode already has pane boundary affordances; layering Termy's active-pane
                // outline on top creates a second full-frame box around the active pane.
                let pane_active_border_alpha = effective_pane_focus_active_border_alpha(
                    raw_pane_active_border_alpha,
                    self.runtime_uses_tmux(),
                    self.tmux_show_active_pane_border,
                );
                let pane_focus_target_bg = colors.background;
                // `sync_terminal_size` refreshes this once for active panes at
                // the start of the render pass, avoiding another nested pair
                // of terminal locks here.
                let alternate_screen_mode = pane.last_alternate_screen.get();
                let pane_cache_key = self.pane_render_cache_key(
                    is_active_pane,
                    alternate_screen_mode,
                    search_active,
                    cell_color_transform,
                    effective_background_opacity,
                );
                let (pane_display_offset, _) = terminal.scroll_state();
                let pane_search_results = if search_active && is_active_pane {
                    Some(self.search_state.results())
                } else {
                    None
                };
                let pane_build_context = PaneCellBuildContext {
                    colors: &colors,
                    effective_background_opacity,
                    background_opacity_cells: self.background_opacity_cells,
                    cell_color_transform,
                    pane_focus_target_bg,
                    terminal_surface_bg,
                    selection_range: pane_cache_key.selection_range,
                    pane_search_results,
                };
                #[cfg_attr(not(debug_assertions), allow(unused_variables))]
                let (pane_cells, cache_strategy, paint_damage, paint_cache) = {
                    let mut pane_render_cache = pane.render_cache.borrow_mut();
                    let paint_cache = pane_render_cache.paint_cache.clone();
                    let (pane_cells, cache_strategy, paint_damage) = self.update_pane_render_cache(
                        terminal,
                        cols,
                        rows,
                        pane_display_offset,
                        &mut pane_render_cache,
                        pane_cache_key.clone(),
                        pane_build_context,
                        #[cfg(debug_assertions)]
                        &mut render_pass_cache_counts,
                    );
                    (pane_cells, cache_strategy, paint_damage, paint_cache)
                };
                #[cfg(debug_assertions)]
                render_pass_cache_counts.record(cache_strategy);

                if is_active_pane {
                    terminal_display_offset = pane_display_offset;
                }

                let hovered_link_range = if is_active_pane {
                    self.hovered_link
                        .as_ref()
                        .map(|link| (link.start_row, link.start_col, link.end_row, link.end_col))
                } else {
                    None
                };
                // Keep cursor state out of cached cells so blink/overlay redraws don't force
                // full cell-buffer rebuilds.
                let pane_cursor_state = cursor_state_with_preview(
                    self.pending_cursor_move_preview.as_ref(),
                    pane.id.as_str(),
                    cursor_state_for_pane(
                        terminal,
                        pane_display_offset,
                        is_active_pane,
                        cols,
                        rows,
                    ),
                    is_active_pane,
                    cols,
                    rows,
                );
                let (cursor_cell, cursor_paint_visible, pane_cursor_style) = match pane_cursor_state
                {
                    Some(cursor) => (Some((cursor.col, cursor.row)), cursor_visible, cursor.style),
                    None => (None, false, configured_cursor_style),
                };

                let pane_cell_size = self
                    .cached_cell_size_for_font_size(pane_font_size)
                    .unwrap_or(layout_cell_size);
                let terminal_grid = self.build_terminal_grid_from_cache(
                    pane_cells,
                    paint_cache,
                    paint_damage,
                    pane_cell_size,
                    cols,
                    rows,
                    &colors,
                    hovered_link_range,
                    font_family.clone(),
                    pane_font_size,
                    pane_cursor_style,
                    cursor_cell,
                    cursor_paint_visible,
                    terminal_surface_bg,
                );
                let (kitty_below_text, kitty_above_text) = {
                    let mut pane_render_cache = pane.render_cache.borrow_mut();
                    kitty_graphics_layers(
                        terminal.kitty_graphics_placements(),
                        pane_cell_size,
                        &mut pane_render_cache.kitty_images,
                        KittyGraphicsSelectionPaint {
                            pane_id: pane.id.as_str(),
                            display_offset: pane_display_offset,
                            selection_range: pane_cache_key.selection_range,
                            explicit: self.kitty_image_selection.as_ref(),
                            color: colors.cursor,
                        },
                    )
                };

                let Some(pane_layout) = self.terminal_pane_layout(active_tab, pane, content_bounds)
                else {
                    continue;
                };
                let pane_frame_left = pane_layout.frame.origin_x;
                let pane_frame_top = pane_layout.frame.origin_y;
                let pane_frame_width = pane_layout.frame.width;
                let pane_frame_height = pane_layout.frame.height;
                let pane_left = pane_layout.content_frame.origin_x;
                let pane_top = pane_layout.content_frame.origin_y;
                let pane_width = pane_layout.content_frame.width;
                let pane_height = pane_layout.content_frame.height;

                let link_hovered = is_active_pane && self.hovered_link.is_some();
                let pane_progress_loader = self.pane_progress_loader_element(pane.progress_state);
                pane_layers.push(
                    div()
                        .id(pane.cached_element_ids.pane.clone())
                        .absolute()
                        .left(px(pane_left))
                        .top(px(pane_top))
                        .w(px(pane_width))
                        .h(px(pane_height))
                        .overflow_hidden()
                        .cursor_text()
                        .when(link_hovered, |el| el.cursor_pointer())
                        .children(kitty_below_text)
                        .child(terminal_grid)
                        .children(kitty_above_text)
                        .children(pane_progress_loader)
                        .into_any_element(),
                );

                if multi_pane && pane_active_border_alpha > f32::EPSILON {
                    let mut border = blend_rgb_only(colors.cursor, colors.foreground, 0.32);
                    border.a =
                        self.scaled_chrome_alpha((pane_active_border_alpha * 0.72).max(0.16));
                    let border_hsla: gpui::Hsla = border.into();
                    pane_focus_accents.push(
                        div()
                            .id(pane.cached_element_ids.focus_accent.clone())
                            .absolute()
                            .left(px(pane_frame_left))
                            .top(px(pane_frame_top))
                            .w(px(pane_frame_width))
                            .h(px(pane_frame_height))
                            .border_1()
                            .border_color(border_hsla)
                            .into_any_element(),
                    );
                }

                if pane.degraded {
                    // Hydration degraded panes still function, but this marker makes
                    // the warning state persistent until the next successful snapshot.
                    let degraded_accent = gpui::Hsla {
                        h: 0.09,
                        s: 0.92,
                        l: 0.58,
                        a: self.scaled_chrome_alpha(0.68),
                    };
                    pane_focus_accents.push(
                        div()
                            .id(pane.cached_element_ids.degraded_accent.clone())
                            .absolute()
                            .left(px(pane_frame_left))
                            .top(px(pane_frame_top))
                            .w(px(pane_frame_width))
                            .h(px(pane_frame_height))
                            .border_1()
                            .border_color(degraded_accent)
                            .into_any_element(),
                    );
                }

                if multi_pane && self.runtime_kind() == RuntimeKind::Native {
                    // Pane-move handle at the top center, shown only on hover or drag.
                    let is_drag_source = self
                        .pane_move_drag
                        .as_ref()
                        .is_some_and(|drag| drag.pane_id == pane.id);
                    let handle_width = PANE_DRAG_HANDLE_WIDTH.min(pane_frame_width * 0.5);
                    let handle_left = pane_frame_left + (pane_frame_width - handle_width) * 0.5;
                    let handle_top = pane_frame_top + PANE_DRAG_HANDLE_INSET_Y;
                    let mut grip_color = colors.foreground;
                    grip_color.a = self.scaled_chrome_alpha(0.55);
                    let idle_grip_color = if is_drag_source {
                        grip_color
                    } else {
                        gpui::Rgba {
                            a: 0.0,
                            ..grip_color
                        }
                    };
                    let group_name = pane.cached_element_ids.drag_handle.clone();
                    let drag_pane_id = pane.id.clone();
                    pane_drag_handles.push(
                        div()
                            .id(pane.cached_element_ids.drag_handle.clone())
                            .group(group_name.clone())
                            .absolute()
                            .left(px(handle_left))
                            .top(px(handle_top))
                            .w(px(handle_width))
                            .h(px(PANE_DRAG_HANDLE_HEIGHT))
                            .cursor(gpui::CursorStyle::OpenHand)
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(3.0))
                            .children((0..3).map(|_| {
                                let group_name = group_name.clone();
                                div()
                                    .w(px(2.5))
                                    .h(px(2.5))
                                    .rounded_full()
                                    .bg(idle_grip_color)
                                    .group_hover(group_name, move |style| style.bg(grip_color))
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, event: &MouseDownEvent, _window, cx| {
                                    view.begin_pane_move_drag_from_handle(
                                        drag_pane_id.clone(),
                                        event.position,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                }),
                            )
                            .into_any_element(),
                    );
                }

                if let Some(drag) = self.pane_move_drag.as_ref().filter(|drag| drag.active) {
                    if drag.pane_id == pane.id {
                        // Mark the pane being dragged so its origin stays
                        // readable while the pointer roams.
                        let mut source_border = colors.foreground;
                        source_border.a = self.scaled_chrome_alpha(0.45);
                        let mut source_fill = colors.foreground;
                        source_fill.a = self.scaled_chrome_alpha(0.05);
                        pane_drop_overlays.push(
                            div()
                                .absolute()
                                .left(px(pane_frame_left))
                                .top(px(pane_frame_top))
                                .w(px(pane_frame_width))
                                .h(px(pane_frame_height))
                                .bg(source_fill)
                                .border_1()
                                .border_color(source_border)
                                .into_any_element(),
                        );
                    }
                    if let Some(target) = drag
                        .drop_target
                        .as_ref()
                        .filter(|target| target.pane_id == pane.id)
                    {
                        // Placement preview: the half (split) or whole pane
                        // (swap) the dragged pane would occupy on drop.
                        let (region_left, region_top, region_width, region_height) =
                            match target.region {
                                PaneDropRegion::Center => (
                                    pane_frame_left,
                                    pane_frame_top,
                                    pane_frame_width,
                                    pane_frame_height,
                                ),
                                PaneDropRegion::Left => (
                                    pane_frame_left,
                                    pane_frame_top,
                                    pane_frame_width * 0.5,
                                    pane_frame_height,
                                ),
                                PaneDropRegion::Right => (
                                    pane_frame_left + pane_frame_width * 0.5,
                                    pane_frame_top,
                                    pane_frame_width * 0.5,
                                    pane_frame_height,
                                ),
                                PaneDropRegion::Top => (
                                    pane_frame_left,
                                    pane_frame_top,
                                    pane_frame_width,
                                    pane_frame_height * 0.5,
                                ),
                                PaneDropRegion::Bottom => (
                                    pane_frame_left,
                                    pane_frame_top + pane_frame_height * 0.5,
                                    pane_frame_width,
                                    pane_frame_height * 0.5,
                                ),
                            };
                        let mut placement_fill = colors.cursor;
                        placement_fill.a = self.scaled_chrome_accent_alpha(0.16);
                        let mut placement_outline = colors.cursor;
                        placement_outline.a = self.scaled_chrome_accent_alpha(0.78);
                        pane_drop_overlays.push(
                            div()
                                .absolute()
                                .left(px(region_left))
                                .top(px(region_top))
                                .w(px(region_width))
                                .h(px(region_height))
                                .rounded(px(4.0))
                                .bg(placement_fill)
                                .border_1()
                                .border_color(placement_outline)
                                .into_any_element(),
                        );
                    }
                }
            }

            for divider in pane_divider_layouts {
                let is_dragging = self.pane_resize_drag.as_ref().is_some_and(|drag| {
                    drag.pane_id == divider.pane_id && drag.edge == divider.edge
                });
                let is_hovered = self.hovered_pane_divider.as_ref().is_some_and(|hover| {
                    hover.pane_id == divider.pane_id && hover.edge == divider.edge
                });
                let pane_id = divider.pane_id.clone();
                let axis = divider.axis;
                let edge = divider.edge;
                let cursor_color_hsla: gpui::Hsla = colors.cursor.into();
                let blocked_color = gpui::Hsla {
                    h: 0.1,
                    s: 0.88,
                    l: 0.58,
                    a: 0.96,
                };
                let track_thickness = if is_dragging && self.pane_resize_blocked {
                    4.0
                } else if is_dragging {
                    3.0
                } else if is_hovered {
                    2.0
                } else {
                    1.0
                };
                let track_color = if is_dragging && self.pane_resize_blocked {
                    blocked_color
                } else if is_dragging || is_hovered {
                    cursor_color_hsla
                } else {
                    divider_line_color
                };

                pane_dividers.push(
                    div()
                        .absolute()
                        .left(px(if divider.axis == PaneResizeAxis::Horizontal {
                            divider.line_frame.origin_x
                                - ((track_thickness - divider.line_frame.width) * 0.5)
                        } else {
                            divider.line_frame.origin_x
                        }))
                        .top(px(if divider.axis == PaneResizeAxis::Vertical {
                            divider.line_frame.origin_y
                                - ((track_thickness - divider.line_frame.height) * 0.5)
                        } else {
                            divider.line_frame.origin_y
                        }))
                        .w(px(if divider.axis == PaneResizeAxis::Horizontal {
                            track_thickness
                        } else {
                            divider.line_frame.width
                        }))
                        .h(px(if divider.axis == PaneResizeAxis::Vertical {
                            track_thickness
                        } else {
                            divider.line_frame.height
                        }))
                        .bg(track_color)
                        .into_any_element(),
                );
                pane_resize_handles.push(
                    div()
                        .id(divider.handle_id.clone())
                        .absolute()
                        .left(px(divider.hit_frame.origin_x))
                        .top(px(divider.hit_frame.origin_y))
                        .w(px(divider.hit_frame.width))
                        .h(px(divider.hit_frame.height))
                        .when(axis == PaneResizeAxis::Horizontal, |el| {
                            el.cursor_col_resize()
                        })
                        .when(axis == PaneResizeAxis::Vertical, |el| {
                            el.cursor_row_resize()
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, event: &MouseDownEvent, _window, cx| {
                                view.begin_pane_resize_drag(
                                    pane_id.as_str(),
                                    axis,
                                    edge,
                                    event.position,
                                );
                                cx.stop_propagation();
                            }),
                        )
                        .into_any_element(),
                );
            }
        }

        if self.tabs.is_empty()
            && let Some(content_bounds) = self.terminal_content_bounds(window)
        {
            let mut empty_text = colors.foreground;
            empty_text.a = self.scaled_chrome_alpha(0.52);
            pane_layers.push(
                div()
                    .id("empty-workspace")
                    .absolute()
                    .left(px(content_bounds.origin_x))
                    .top(px(content_bounds.origin_y))
                    .w(px(content_bounds.width))
                    .h(px(content_bounds.height))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(ui_font_family.clone())
                    .text_size(px(13.0))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(empty_text)
                    .child("No tabs found in here")
                    .into_any_element(),
            );
        }

        if self
            .tab_strip
            .switch_hints
            .animation_active(now, self.tab_switch_hints_blocked())
        {
            self.schedule_tab_switch_hint_animation(cx);
        }
        #[cfg(debug_assertions)]
        self.record_render_metrics_for_pass(render_pass_cache_counts);

        let focus_handle = self.focus_handle.clone();
        let tabbar_bg = terminal_surface_bg;
        let show_tab_strip_chrome = self.should_render_tab_strip_chrome();
        let titlebar_height = Self::window_titlebar_height_for(false, show_tab_strip_chrome);
        let vertical_tabs = self.tab_strip_orientation()
            == crate::terminal_view::tab_strip::state::TabStripOrientation::Vertical;
        let show_horizontal_tabbar = show_tab_strip_chrome && !vertical_tabs;
        let tabs_row = show_horizontal_tabbar
            .then(|| self.render_tab_strip(window, &colors, &ui_font_family, tabbar_bg, cx));
        let tab_sidebar = (vertical_tabs && show_tab_strip_chrome)
            .then(|| self.render_tab_sidebar(window, &colors, &ui_font_family, tabbar_bg, cx));
        let workspace_sidebar = self
            .workspace_sidebar_visible()
            .then(|| self.render_workspace_sidebar(&colors, &ui_font_family, tabbar_bg, cx));
        let workspace_sidebar_overlay = self.workspace_sidebar_overlay_visible().then(|| {
            self.render_workspace_sidebar_overlay(&colors, &ui_font_family, tabbar_bg, cx)
        });
        let workspace_sidebar_edge_peek = self
            .workspace_sidebar_edge_peek_enabled()
            .then(|| self.render_workspace_sidebar_edge_peek_target(cx));
        self.sync_browser_tab_titles();
        self.sync_browser_webviews(window, cx);
        let hidden_titlebar_branding = Self::should_render_hidden_titlebar_branding(
            self.auto_hide_tabbar,
            self.tabs.len(),
            self.effective_tab_bar_visibility(),
            self.show_termy_in_titlebar,
        )
        .then(|| {
            self.render_titlebar_branding(window, &colors, &ui_font_family, tabbar_bg, false, cx)
        })
        .flatten();
        if self.terminal_scrollbar_mode() == ui_scrollbar::ScrollbarVisibilityMode::OnScroll
            && !self.terminal_scrollbar_animation_active
            && self.terminal_scrollbar_needs_animation(Instant::now())
        {
            self.start_terminal_scrollbar_animation(cx);
        }
        let terminal_surface = self
            .active_terminal_pane_layout(window)
            .map(|pane_layout| pane_layout.scrollbar_surface);
        let terminal_scrollbar_layout = terminal_surface.and_then(|surface| {
            self.terminal_scrollbar_layout_for_track(surface.height)
                .map(|layout| (surface, layout))
        });
        if terminal_scrollbar_layout.is_none() {
            self.clear_terminal_scrollbar_marker_cache();
        }
        let terminal_scrollbar_overlay = terminal_scrollbar_layout.and_then(|(surface, layout)| {
            self.render_terminal_scrollbar_overlay(surface, layout, terminal_display_offset > 0)
        });
        let terminal_grid_layer = div()
            .relative()
            .w_full()
            .h_full()
            .children(pane_layers)
            .children(pane_dividers)
            .children(pane_resize_handles)
            .children(pane_focus_accents)
            .children(pane_drag_handles)
            .children(pane_drop_overlays)
            .into_any_element();
        let inspector_panel = self.render_inspector_panel(cx);
        let has_active_inline = self.has_active_inline_input();
        let ime_focus_handle = self.focus_handle.clone();
        let ime_view = cx.entity();
        let ime_input_layer = canvas(
            move |_bounds, _window, _cx| {},
            move |bounds, _, window, cx| {
                if !has_active_inline {
                    window.handle_input(
                        &ime_focus_handle,
                        ElementInputHandler::new(bounds, ime_view.clone()),
                        cx,
                    );
                }
            },
        )
        .absolute()
        .size_full()
        .into_any_element();
        let ime_preedit_overlay = self.ime_marked_text.as_ref().and_then(|text| {
            if text.is_empty() {
                return None;
            }
            let bounds = self.ime_cursor_bounds()?;
            let fg_color: gpui::Hsla = self.colors.foreground.into();
            let bg_color: gpui::Hsla = self.colors.background.into();
            Some(
                div()
                    .absolute()
                    .left(bounds.origin.x)
                    .top(bounds.origin.y)
                    .h(bounds.size.height)
                    .bg(bg_color)
                    .border_b_1()
                    .border_color(fg_color)
                    .text_color(fg_color)
                    .font_family(font_family.clone())
                    .text_size(font_size)
                    .line_height(bounds.size.height)
                    .child(text.clone())
                    .into_any_element(),
            )
        });
        let overlay_view = self.ensure_overlay_view(cx);
        let key_context = if self.plugin_ui.is_some() {
            "PluginUI"
        } else if self.has_active_inline_input() {
            "Terminal InlineInput"
        } else {
            "Terminal"
        };
        let titlebar_element: Option<AnyElement> = (titlebar_height > 0.0).then(|| {
            // The root already paints the shared translucent window background.
            // Repainting that same fill on the titlebar darkens the top strip
            // relative to the terminal content.
            let titlebar_container = div()
                .id("titlebar")
                .w_full()
                .h(px(titlebar_height))
                .flex_none()
                .relative()
                .flex()
                .items_center()
                .on_mouse_move(cx.listener(Self::handle_titlebar_tab_strip_mouse_move));

            titlebar_container
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(Self::handle_unified_titlebar_mouse_down),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(Self::handle_unified_titlebar_mouse_up),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(Self::handle_unified_titlebar_mouse_up),
                )
                .child(
                    div()
                        .w_full()
                        .h_full()
                        .flex()
                        .items_end()
                        .mt(px(TOP_STRIP_CONTENT_OFFSET_Y))
                        .children(tabs_row)
                        .children(hidden_titlebar_branding),
                )
                .into_any()
        });
        let mut root_bg = colors.background;
        root_bg.a = self.scaled_background_alpha(root_bg.a);

        let root = div()
            .id("termy-root")
            .flex()
            .flex_col()
            .size_full()
            .bg(root_bg)
            .font_family(ui_font_family)
            .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                if matches!(
                    event.button,
                    MouseButton::Left | MouseButton::Middle | MouseButton::Right
                ) {
                    this.handle_global_mouse_up_event(event, cx);
                }
                if event.button == MouseButton::Left {
                    this.disarm_titlebar_window_move();
                    this.commit_tab_drag(cx);
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.handle_global_mouse_move_event(event, window, cx);
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_workspace_sidebar_resize_drag() {
                        cx.notify();
                    }
                    this.disarm_titlebar_window_move();
                    this.commit_tab_drag(cx);
                }),
            )
            .when(self.workspace_sidebar_resize_drag_active(), |s| {
                s.cursor_col_resize()
            })
            .children(titlebar_element)
            .child(
                div()
                    .id("terminal")
                    .track_focus(&focus_handle)
                    .key_context(key_context)
                    .on_action(cx.listener(Self::handle_toggle_command_palette_action))
                    .on_action(cx.listener(Self::handle_import_colors_action))
                    .on_action(cx.listener(Self::handle_prettify_config_action))
                    .on_action(cx.listener(Self::handle_switch_theme_action))
                    .on_action(cx.listener(Self::handle_app_info_action))
                    .on_action(cx.listener(Self::handle_restart_app_action))
                    .on_action(cx.listener(Self::handle_rename_tab_action))
                    .on_action(cx.listener(Self::handle_check_for_updates_action))
                    .on_action(cx.listener(Self::handle_toggle_workspace_sidebar_action))
                    .on_action(cx.listener(Self::handle_new_tab_action))
                    .on_action(cx.listener(Self::handle_close_tab_action))
                    .on_action(cx.listener(Self::handle_close_pane_or_tab_action))
                    .on_action(cx.listener(Self::handle_move_tab_left_action))
                    .on_action(cx.listener(Self::handle_move_tab_right_action))
                    .on_action(cx.listener(Self::handle_switch_tab_left_action))
                    .on_action(cx.listener(Self::handle_switch_tab_right_action))
                    .on_action(cx.listener(Self::handle_cycle_tabs_action))
                    .on_action(cx.listener(Self::handle_switch_to_tab_1_action))
                    .on_action(cx.listener(Self::handle_switch_to_tab_2_action))
                    .on_action(cx.listener(Self::handle_switch_to_tab_3_action))
                    .on_action(cx.listener(Self::handle_switch_to_tab_4_action))
                    .on_action(cx.listener(Self::handle_switch_to_tab_5_action))
                    .on_action(cx.listener(Self::handle_switch_to_tab_6_action))
                    .on_action(cx.listener(Self::handle_switch_to_tab_7_action))
                    .on_action(cx.listener(Self::handle_switch_to_tab_8_action))
                    .on_action(cx.listener(Self::handle_switch_to_tab_9_action))
                    .on_action(cx.listener(Self::handle_manage_tmux_sessions_action))
                    .on_action(cx.listener(Self::handle_manage_saved_layouts_action))
                    .on_action(cx.listener(Self::handle_run_task_action))
                    .on_action(cx.listener(Self::handle_run_named_task_action))
                    .on_action(cx.listener(Self::handle_run_plugin_command_action))
                    .on_action(cx.listener(Self::handle_split_pane_vertical_action))
                    .on_action(cx.listener(Self::handle_split_pane_horizontal_action))
                    .on_action(cx.listener(Self::handle_close_pane_action))
                    .on_action(cx.listener(Self::handle_focus_pane_next_action))
                    .on_action(cx.listener(Self::handle_focus_pane_left_action))
                    .on_action(cx.listener(Self::handle_focus_pane_right_action))
                    .on_action(cx.listener(Self::handle_focus_pane_up_action))
                    .on_action(cx.listener(Self::handle_focus_pane_down_action))
                    .on_action(cx.listener(Self::handle_focus_pane_previous_action))
                    .on_action(cx.listener(Self::handle_resize_pane_left_action))
                    .on_action(cx.listener(Self::handle_resize_pane_right_action))
                    .on_action(cx.listener(Self::handle_resize_pane_up_action))
                    .on_action(cx.listener(Self::handle_resize_pane_down_action))
                    .on_action(cx.listener(Self::handle_toggle_pane_zoom_action))
                    .on_action(cx.listener(Self::handle_minimize_window_action))
                    .on_action(cx.listener(Self::handle_copy_action))
                    .on_action(cx.listener(Self::handle_paste_action))
                    .on_action(cx.listener(Self::handle_zoom_in_action))
                    .on_action(cx.listener(Self::handle_zoom_out_action))
                    .on_action(cx.listener(Self::handle_zoom_reset_action))
                    .on_action(cx.listener(Self::handle_quit_action))
                    .on_action(cx.listener(Self::handle_open_search_action))
                    .on_action(cx.listener(Self::handle_close_search_action))
                    .on_action(cx.listener(Self::handle_search_next_action))
                    .on_action(cx.listener(Self::handle_search_previous_action))
                    .on_action(cx.listener(Self::handle_toggle_search_case_sensitive_action))
                    .on_action(cx.listener(Self::handle_toggle_search_regex_action))
                    .when(self.install_cli_available(), |s| {
                        s.on_action(cx.listener(Self::handle_install_cli_action))
                    })
                    .on_action(cx.listener(Self::handle_toggle_tab_bar_visibility_action))
                    .on_action(cx.listener(Self::handle_toggle_inspector_action))
                    .on_action(cx.listener(Self::handle_inline_backspace_action))
                    .on_action(cx.listener(Self::handle_inline_delete_action))
                    .on_action(cx.listener(Self::handle_inline_move_left_action))
                    .on_action(cx.listener(Self::handle_inline_move_right_action))
                    .on_action(cx.listener(Self::handle_inline_select_left_action))
                    .on_action(cx.listener(Self::handle_inline_select_right_action))
                    .on_action(cx.listener(Self::handle_inline_select_all_action))
                    .on_action(cx.listener(Self::handle_inline_move_to_start_action))
                    .on_action(cx.listener(Self::handle_inline_move_to_end_action))
                    .on_action(cx.listener(Self::handle_inline_delete_word_backward_action))
                    .on_action(cx.listener(Self::handle_inline_delete_word_forward_action))
                    .on_action(cx.listener(Self::handle_inline_delete_to_start_action))
                    .on_action(cx.listener(Self::handle_inline_delete_to_end_action))
                    .on_key_down(cx.listener(Self::handle_key_down))
                    .on_key_up(cx.listener(Self::handle_key_up))
                    .on_modifiers_changed(cx.listener(Self::handle_modifiers_changed))
                    .relative()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("terminal-content")
                            .flex()
                            .w_full()
                            .h_full()
                            .children(workspace_sidebar)
                            .child(
                                div()
                                    .id("terminal-pane")
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .h_full()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .id("terminal-surface")
                                            .relative()
                                            .flex_1()
                                            .h_full()
                                            .overflow_hidden()
                                            .cursor_text()
                                            .on_scroll_wheel(
                                                cx.listener(Self::handle_terminal_scroll_wheel),
                                            )
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(Self::handle_mouse_down),
                                            )
                                            .on_mouse_down(
                                                MouseButton::Middle,
                                                cx.listener(Self::handle_mouse_down),
                                            )
                                            .on_mouse_down(
                                                MouseButton::Right,
                                                cx.listener(Self::handle_mouse_down),
                                            )
                                            .on_mouse_move(cx.listener(Self::handle_mouse_move))
                                            .on_mouse_up(
                                                MouseButton::Left,
                                                cx.listener(Self::handle_mouse_up),
                                            )
                                            .on_mouse_up(
                                                MouseButton::Middle,
                                                cx.listener(Self::handle_mouse_up),
                                            )
                                            .on_mouse_up(
                                                MouseButton::Right,
                                                cx.listener(Self::handle_mouse_up),
                                            )
                                            .when_some(
                                                self.pane_resize_drag.as_ref().map(|d| d.axis).or(
                                                    self.hovered_pane_divider
                                                        .as_ref()
                                                        .map(|h| h.axis),
                                                ),
                                                |s, axis| match axis {
                                                    PaneResizeAxis::Horizontal => {
                                                        s.cursor_col_resize()
                                                    }
                                                    PaneResizeAxis::Vertical => {
                                                        s.cursor_row_resize()
                                                    }
                                                },
                                            )
                                            .when(self.pane_move_drag_active(), |s| {
                                                s.cursor(gpui::CursorStyle::ClosedHand)
                                            })
                                            .font_family(font_family)
                                            .text_size(font_size)
                                            .child(ime_input_layer)
                                            .child(terminal_grid_layer)
                                            .children(ime_preedit_overlay)
                                            .children(terminal_scrollbar_overlay),
                                    )
                                    .children(inspector_panel),
                            )
                            .children(tab_sidebar),
                    ),
            )
            .children(workspace_sidebar_edge_peek)
            .children(workspace_sidebar_overlay)
            .child(overlay_view);

        #[cfg(target_os = "macos")]
        let root = if self.native_file_drop_enabled {
            root
        } else {
            root.on_drop(cx.listener(Self::handle_file_drop))
        };

        #[cfg(not(target_os = "macos"))]
        let root = root.on_drop(cx.listener(Self::handle_file_drop));

        #[cfg(debug_assertions)]
        self.maybe_emit_render_metrics_log(Instant::now());

        if let Some(started_at) = view_build_started_at {
            self.debug_overlay_stats
                .record_view_build_duration(started_at.elapsed());
        }

        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_progress_loader_fill_uses_percentage_width() {
        assert_eq!(
            terminal_progress_loader_fill(ProgressState::InProgress(42), 0),
            Some(TerminalProgressLoaderFill {
                left_ratio: 0.0,
                width_ratio: 0.42
            })
        );
    }

    #[test]
    fn terminal_progress_loader_fill_ping_pongs_indeterminate() {
        let fill = terminal_progress_loader_fill(ProgressState::Indeterminate, 0).expect("fill");
        assert_eq!(fill.width_ratio, TERMINAL_PROGRESS_INDETERMINATE_WIDTH);
        assert_close(fill.left_ratio, 0.0);

        let fill = terminal_progress_loader_fill(
            ProgressState::Indeterminate,
            TERMINAL_PROGRESS_INDETERMINATE_CYCLE_MS / 2,
        )
        .expect("fill");
        assert_close(fill.left_ratio, 1.0 - TERMINAL_PROGRESS_INDETERMINATE_WIDTH);

        let fill = terminal_progress_loader_fill(
            ProgressState::Indeterminate,
            TERMINAL_PROGRESS_INDETERMINATE_CYCLE_MS,
        )
        .expect("fill");
        assert_close(fill.left_ratio, 0.0);
    }

    #[test]
    fn terminal_progress_loader_fill_is_symmetric_during_ping_pong() {
        let outbound = terminal_progress_loader_fill(
            ProgressState::Indeterminate,
            TERMINAL_PROGRESS_INDETERMINATE_CYCLE_MS / 4,
        )
        .expect("outbound fill");
        let inbound = terminal_progress_loader_fill(
            ProgressState::Indeterminate,
            TERMINAL_PROGRESS_INDETERMINATE_CYCLE_MS * 3 / 4,
        )
        .expect("inbound fill");
        assert_close(outbound.left_ratio, inbound.left_ratio);
    }

    #[test]
    fn terminal_progress_loader_fill_hides_clear_state() {
        assert_eq!(terminal_progress_loader_fill(ProgressState::Clear, 0), None);
    }

    #[test]
    fn context_menu_height_scrolls_long_plugin_lists_inside_the_viewport() {
        assert_eq!(context_menu_visible_height(900.0, Some(600.0), 30.0), 584.0);
        assert_eq!(context_menu_visible_height(300.0, Some(600.0), 30.0), 300.0);
        assert_eq!(context_menu_visible_height(900.0, None, 30.0), 900.0);
    }

    #[test]
    fn context_menu_height_keeps_one_row_visible_in_tiny_viewports() {
        assert_eq!(context_menu_visible_height(900.0, Some(20.0), 30.0), 38.0);
    }

    fn assert_close(left: f32, right: f32) {
        assert!(
            (left - right).abs() < 0.0001,
            "expected {left} to be close to {right}"
        );
    }

    #[test]
    fn update_dialog_layout_floats_below_titlebar_without_reserving_space() {
        assert_eq!(
            TerminalView::update_banner_layout_for(true, true),
            Some(UpdateBannerLayout {
                overlay_top: TerminalView::titlebar_height() + 10.0,
                overlay_left: 0.0,
            })
        );
    }

    fn test_render_cell(col: usize, row: usize, c: char) -> CellRenderInfo {
        CellRenderInfo {
            col,
            row,
            char: c,
            fg: gpui::Hsla::transparent_black(),
            bg: gpui::Hsla::transparent_black(),
            uses_terminal_default_bg: false,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            render_text: true,
            selected: false,
            search_current: false,
            search_match: false,
        }
    }

    fn test_build_context(opacity: f32) -> PaneCellBuildContext<'static> {
        test_build_context_with_background_cells(opacity, false)
    }

    fn test_build_context_with_background_cells(
        opacity: f32,
        background_opacity_cells: bool,
    ) -> PaneCellBuildContext<'static> {
        static COLORS: std::sync::LazyLock<TerminalColors> =
            std::sync::LazyLock::new(TerminalColors::default);
        PaneCellBuildContext {
            colors: &COLORS,
            effective_background_opacity: opacity,
            background_opacity_cells,
            cell_color_transform: CellColorTransform::default(),
            pane_focus_target_bg: COLORS.background,
            terminal_surface_bg: COLORS.background,
            selection_range: None,
            pane_search_results: None,
        }
    }

    fn test_build_context_with_transform(
        opacity: f32,
        cell_color_transform: CellColorTransform,
        pane_focus_target_bg: gpui::Rgba,
        terminal_surface_bg: gpui::Rgba,
    ) -> PaneCellBuildContext<'static> {
        static COLORS: std::sync::LazyLock<TerminalColors> =
            std::sync::LazyLock::new(TerminalColors::default);
        PaneCellBuildContext {
            colors: &COLORS,
            effective_background_opacity: opacity,
            background_opacity_cells: false,
            cell_color_transform,
            pane_focus_target_bg,
            terminal_surface_bg,
            selection_range: None,
            pane_search_results: None,
        }
    }

    fn test_term_cell(
        fg: AnsiColor,
        bg: AnsiColor,
        flags: Flags,
    ) -> alacritty_terminal::term::cell::Cell {
        alacritty_terminal::term::cell::Cell {
            fg,
            bg,
            flags,
            ..alacritty_terminal::term::cell::Cell::default()
        }
    }

    #[test]
    fn cell_text_attributes_preserve_sgr_flags() {
        let attributes =
            cell_text_attributes(Flags::BOLD | Flags::ITALIC | Flags::UNDERLINE | Flags::STRIKEOUT);

        assert!(attributes.bold);
        assert!(attributes.italic);
        assert!(attributes.underline);
        assert!(attributes.strikethrough);
    }

    fn tmux_test_pane(id: &str, left: u16, top: u16, cols: u16, rows: u16) -> TerminalPane {
        let size = TerminalSize {
            cols,
            rows,
            ..TerminalSize::default()
        };
        TerminalPane {
            id: id.to_string(),
            left,
            top,
            width: cols,
            height: rows,
            pane_zoom_steps: 0,
            degraded: false,
            tmux_mouse_mode: None,
            progress_state: ProgressState::default(),
            content: PaneContent::Terminal(Terminal::new_tmux(
                size,
                TerminalOptions {
                    scrollback_history: 128,
                    ..TerminalOptions::default()
                },
            )),
            render_cache: std::cell::RefCell::new(TerminalPaneRenderCache::default()),
            last_alternate_screen: std::cell::Cell::new(false),
            cached_element_ids: PaneCachedElementIds::new(id),
        }
    }

    fn test_render_rows(rows: Vec<Vec<CellRenderInfo>>) -> PaneRenderCells {
        Arc::new(rows.into_iter().map(Arc::new).collect())
    }

    #[test]
    fn resolved_cursor_state_for_pane_keeps_terminal_hidden_cursor_hidden() {
        let resolved = filtered_cursor_state(None, 0, true, 10, 4);
        assert_eq!(resolved, None);
    }

    #[test]
    fn resolved_cursor_state_for_pane_filters_inactive_scrolled_and_out_of_bounds_cursors() {
        let cursor = TerminalCursorState {
            col: 3,
            row: 1,
            style: TerminalCursorStyle::Line,
        };
        assert_eq!(filtered_cursor_state(Some(cursor), 1, true, 10, 4), None);
        assert_eq!(filtered_cursor_state(Some(cursor), 0, false, 10, 4), None);
        assert_eq!(
            filtered_cursor_state(
                Some(TerminalCursorState {
                    col: 12,
                    row: 1,
                    style: TerminalCursorStyle::Block,
                }),
                0,
                true,
                10,
                4,
            ),
            None
        );
        assert_eq!(
            filtered_cursor_state(
                Some(TerminalCursorState {
                    col: 3,
                    row: 4,
                    style: TerminalCursorStyle::Block,
                }),
                0,
                true,
                10,
                4,
            ),
            None
        );
        assert_eq!(
            filtered_cursor_state(Some(cursor), 0, true, 10, 4),
            Some(cursor)
        );
    }

    #[test]
    fn cursor_state_preview_overrides_until_terminal_catches_up() {
        let preview = PendingCursorMovePreview {
            pane_id: "%pane".to_string(),
            target: CellPos { col: 8, row: 1 },
            style: TerminalCursorStyle::Line,
        };
        let actual = Some(TerminalCursorState {
            col: 3,
            row: 1,
            style: TerminalCursorStyle::Block,
        });

        assert_eq!(
            cursor_state_with_preview(Some(&preview), "%pane", actual, true, 10, 4),
            Some(TerminalCursorState {
                col: 8,
                row: 1,
                style: TerminalCursorStyle::Line,
            })
        );
        assert_eq!(
            cursor_state_with_preview(
                Some(&preview),
                "%pane",
                Some(TerminalCursorState {
                    col: 8,
                    row: 1,
                    style: TerminalCursorStyle::Block,
                }),
                true,
                10,
                4,
            ),
            Some(TerminalCursorState {
                col: 8,
                row: 1,
                style: TerminalCursorStyle::Block,
            })
        );
    }

    #[test]
    fn partial_damage_rebuild_threshold_uses_dirty_cell_coverage() {
        let spans = vec![TerminalDirtySpan {
            row: 0,
            left_col: 0,
            right_col: 49,
        }];
        assert!(partial_damage_should_rebuild_full(&spans, 2, 50));

        let spans = vec![TerminalDirtySpan {
            row: 0,
            left_col: 0,
            right_col: 2,
        }];
        assert!(!partial_damage_should_rebuild_full(&spans, 2, 50));
    }

    #[test]
    fn patch_pane_render_row_updates_only_touched_row_cells() {
        let mut cells = test_render_rows(vec![
            vec![test_render_cell(0, 0, 'a'), test_render_cell(1, 0, 'b')],
            vec![test_render_cell(0, 1, 'c'), test_render_cell(1, 1, 'd')],
            vec![test_render_cell(0, 2, 'e'), test_render_cell(1, 2, 'f')],
        ]);
        let original = cells.clone();

        let rows: &mut Vec<_> = Arc::make_mut(&mut cells);
        let patched = patch_pane_render_row(rows, 3, 2, 1, 0, 1, |col| {
            test_render_cell(col, 1, if col == 0 { 'w' } else { 'x' })
        });

        assert_eq!(patched, 2);
        assert!(Arc::ptr_eq(&original[0], &cells[0]));
        assert!(!Arc::ptr_eq(&original[1], &cells[1]));
        assert!(Arc::ptr_eq(&original[2], &cells[2]));
        assert_eq!(cells[0][0].char, 'a');
        assert_eq!(cells[1][0].char, 'w');
        assert_eq!(cells[1][1].char, 'x');
        assert_eq!(cells[2][1].char, 'f');
    }

    #[test]
    fn patch_pane_render_row_rejects_out_of_bounds_updates() {
        let mut cells = test_render_rows(vec![vec![
            test_render_cell(0, 0, 'a'),
            test_render_cell(1, 0, 'b'),
        ]]);
        let original = cells.clone();
        let rows: &mut Vec<_> = Arc::make_mut(&mut cells);

        assert_eq!(
            patch_pane_render_row(rows, 1, 2, 7, 0, 0, |col| test_render_cell(col, 7, 'x'),),
            0
        );
        assert_eq!(
            patch_pane_render_row(rows, 1, 2, 0, 7, 7, |col| test_render_cell(col, 0, 'x'),),
            0
        );
        assert!(Arc::ptr_eq(&original[0], &cells[0]));
        assert_eq!(cells[0][0].char, 'a');
        assert_eq!(cells[0][1].char, 'b');
    }

    #[test]
    fn terminal_scrollbar_overlay_frame_anchors_to_active_pane_geometry() {
        let surface =
            TerminalScrollbarSurfaceGeometry::new(32.0, 48.0, 640.0, 420.0).expect("surface");

        let frame = terminal_scrollbar_overlay_frame(surface).expect("frame");
        assert_eq!(
            frame.left,
            surface.origin_x + surface.width - TERMINAL_SCROLLBAR_GUTTER_WIDTH
        );
        assert_eq!(frame.top, surface.origin_y);
        assert_eq!(frame.width, TERMINAL_SCROLLBAR_GUTTER_WIDTH);
        assert_eq!(frame.height, surface.height);
    }

    #[test]
    fn terminal_scrollbar_overlay_frame_clamps_when_surface_is_narrower_than_gutter() {
        let surface =
            TerminalScrollbarSurfaceGeometry::new(10.0, 20.0, 6.0, 100.0).expect("surface");

        let frame = terminal_scrollbar_overlay_frame(surface).expect("frame");
        assert_eq!(frame.left, surface.origin_x);
        assert_eq!(frame.top, surface.origin_y);
        assert_eq!(frame.width, surface.width);
        assert_eq!(frame.height, surface.height);
    }

    #[test]
    fn terminal_scrollbar_overlay_frame_uses_scrollbar_surface_width() {
        let surface =
            TerminalScrollbarSurfaceGeometry::new(0.0, 0.0, 1007.0, 809.0).expect("surface");

        let frame = terminal_scrollbar_overlay_frame(surface).expect("frame");
        assert_eq!(frame.left, 1007.0 - TERMINAL_SCROLLBAR_GUTTER_WIDTH);
        assert_eq!(frame.width, TERMINAL_SCROLLBAR_GUTTER_WIDTH);
        assert_eq!(frame.height, surface.height);
    }

    #[test]
    fn apply_cell_color_transform_is_noop_for_zero_factors() {
        let fg = gpui::Rgba {
            r: 0.72,
            g: 0.64,
            b: 0.35,
            a: 0.91,
        };
        let bg = gpui::Rgba {
            r: 0.12,
            g: 0.17,
            b: 0.26,
            a: 0.66,
        };
        let fg_target = gpui::Rgba {
            r: 0.01,
            g: 0.02,
            b: 0.03,
            a: 1.0,
        };
        let bg_target = gpui::Rgba {
            r: 0.98,
            g: 0.97,
            b: 0.96,
            a: 1.0,
        };

        let (next_fg, next_bg) =
            apply_cell_color_transform(fg, bg, CellColorTransform::default(), fg_target, bg_target);

        assert_eq!(next_fg, fg);
        assert_eq!(next_bg, bg);
    }

    #[test]
    fn command_palette_backdrop_transform_uses_soft_spotlight_coefficients() {
        let preset = pane_focus_preset(PaneFocusEffect::SoftSpotlight)
            .expect("soft spotlight preset should exist");
        let transform = command_palette_backdrop_transform();
        let expected_fg = preset.inactive_fg_blend * COMMAND_PALETTE_BACKDROP_STRENGTH;
        let expected_bg = preset.inactive_bg_blend * COMMAND_PALETTE_BACKDROP_STRENGTH;
        let expected_desaturate = preset.inactive_desaturate * COMMAND_PALETTE_BACKDROP_STRENGTH;

        assert!((transform.fg_blend - expected_fg).abs() <= f32::EPSILON);
        assert!((transform.bg_blend - expected_bg).abs() <= f32::EPSILON);
        assert!((transform.desaturate - expected_desaturate).abs() <= f32::EPSILON);
    }

    #[test]
    fn pane_focus_factors_use_immediate_active_and_inactive_states() {
        assert_eq!(pane_focus_factors(true, true), (0.0, 1.0));
        assert_eq!(pane_focus_factors(false, true), (1.0, 0.0));
        assert_eq!(pane_focus_factors(true, false), (0.0, 0.0));
        assert_eq!(pane_focus_factors(false, false), (0.0, 0.0));
    }

    #[test]
    fn terminal_scrollbar_track_width_clamps_to_overlay_frame() {
        assert_eq!(
            terminal_scrollbar_track_width(TERMINAL_SCROLLBAR_TRACK_WIDTH + 2.0),
            TERMINAL_SCROLLBAR_TRACK_WIDTH
        );
        assert_eq!(terminal_scrollbar_track_width(6.0), 6.0);
        assert_eq!(terminal_scrollbar_track_width(-2.0), 0.0);
    }

    #[test]
    fn pane_neighbor_gaps_return_zero_for_adjacent_overlapping_panes() {
        let base = tmux_test_pane("%1", 0, 0, 10, 6);
        let right_adjacent = tmux_test_pane("%2", 10, 2, 5, 2);
        let bottom_adjacent = tmux_test_pane("%3", 2, 6, 3, 3);
        let panes = vec![base, right_adjacent, bottom_adjacent];
        let gaps = TerminalView::pane_neighbor_gaps(&panes[0], &panes);
        assert_eq!(gaps.right_cells, Some(0));
        assert_eq!(gaps.bottom_cells, Some(0));
    }

    #[test]
    fn pane_neighbor_gaps_return_none_without_overlapping_neighbor() {
        let base = tmux_test_pane("%1", 0, 0, 10, 6);
        let separated_right = tmux_test_pane("%2", 10, 6, 5, 3);
        let separated_bottom = tmux_test_pane("%3", 10, 6, 4, 3);
        let panes = vec![base, separated_right, separated_bottom];
        let gaps = TerminalView::pane_neighbor_gaps(&panes[0], &panes);
        assert_eq!(gaps.right_cells, None);
        assert_eq!(gaps.bottom_cells, None);
    }

    #[test]
    fn resolve_cell_colors_scales_only_terminal_default_background_alpha() {
        let context = test_build_context(0.2);

        let default_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Named(NamedColor::Background),
                Flags::empty(),
            ),
            context,
        );
        assert!(default_background.uses_terminal_default_bg);
        assert!((default_background.bg.a - 0.2).abs() <= f32::EPSILON);

        let ansi_black_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Named(NamedColor::Black),
                Flags::empty(),
            ),
            context,
        );
        assert!(!ansi_black_background.uses_terminal_default_bg);
        assert!((ansi_black_background.bg.a - 1.0).abs() <= f32::EPSILON);

        let indexed_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Indexed(232),
                Flags::empty(),
            ),
            context,
        );
        assert!(!indexed_background.uses_terminal_default_bg);
        assert!((indexed_background.bg.a - 1.0).abs() <= f32::EPSILON);

        let rgb_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Spec(alacritty_terminal::vte::ansi::Rgb {
                    r: 12,
                    g: 34,
                    b: 56,
                }),
                Flags::empty(),
            ),
            context,
        );
        assert!(!rgb_background.uses_terminal_default_bg);
        assert!((rgb_background.bg.a - 1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn resolve_cell_colors_scales_explicit_backgrounds_when_cell_opacity_is_enabled() {
        let context = test_build_context_with_background_cells(0.2, true);

        let ansi_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Named(NamedColor::Black),
                Flags::empty(),
            ),
            context,
        );
        assert!(!ansi_background.uses_terminal_default_bg);
        assert!((ansi_background.bg.a - 0.2).abs() <= f32::EPSILON);

        let indexed_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Indexed(232),
                Flags::empty(),
            ),
            context,
        );
        assert!(!indexed_background.uses_terminal_default_bg);
        assert!((indexed_background.bg.a - 0.2).abs() <= f32::EPSILON);

        let rgb_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Spec(alacritty_terminal::vte::ansi::Rgb {
                    r: 12,
                    g: 34,
                    b: 56,
                }),
                Flags::empty(),
            ),
            context,
        );
        assert!(!rgb_background.uses_terminal_default_bg);
        assert!((rgb_background.bg.a - 0.2).abs() <= f32::EPSILON);
    }

    #[test]
    fn resolve_cell_colors_keeps_block_element_backgrounds_opaque() {
        let context = test_build_context_with_background_cells(0.2, true);
        let mut block_cell = test_term_cell(
            AnsiColor::Named(NamedColor::Foreground),
            AnsiColor::Indexed(232),
            Flags::empty(),
        );
        block_cell.c = '\u{2580}';

        let resolved = resolve_cell_colors(&block_cell, context);

        assert!(!resolved.uses_terminal_default_bg);
        assert!((resolved.bg.a - 1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn resolve_cell_colors_classifies_inverse_background_after_swap() {
        let context = test_build_context(0.2);
        let inverse_default_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Background),
                AnsiColor::Named(NamedColor::Red),
                Flags::INVERSE,
            ),
            context,
        );

        assert!(inverse_default_background.uses_terminal_default_bg);
        assert!((inverse_default_background.bg.a - 0.2).abs() <= f32::EPSILON);
    }

    #[test]
    fn resolve_cell_colors_keeps_transformed_default_background_in_sync_with_default_fill() {
        let pane_focus_target_bg = gpui::Rgba {
            r: 0.8,
            g: 0.7,
            b: 0.6,
            a: 1.0,
        };
        let terminal_surface_bg = gpui::Rgba {
            r: 0.1,
            g: 0.2,
            b: 0.3,
            a: 0.4,
        };
        let context = test_build_context_with_transform(
            0.2,
            CellColorTransform {
                fg_blend: 0.0,
                bg_blend: 0.5,
                desaturate: 0.25,
            },
            pane_focus_target_bg,
            terminal_surface_bg,
        );
        let (_, default_bg) = resolved_default_cell_colors(context);
        let resolved = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Named(NamedColor::Background),
                Flags::empty(),
            ),
            context,
        );

        assert!(resolved.uses_terminal_default_bg);
        assert_eq!(resolved.bg, default_bg);
    }

    #[test]
    fn resolve_cell_colors_keeps_opaque_inverse_explicit_background_when_background_opacity_cells_off()
     {
        let context = test_build_context(0.2);
        let inverse_explicit_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Green),
                AnsiColor::Named(NamedColor::Background),
                Flags::INVERSE,
            ),
            context,
        );

        assert!(!inverse_explicit_background.uses_terminal_default_bg);
        assert!((inverse_explicit_background.bg.a - 1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn resolve_cell_colors_scales_inverse_explicit_background_when_cell_opacity_is_enabled() {
        let context = test_build_context_with_background_cells(0.2, true);
        let inverse_explicit_background = resolve_cell_colors(
            &test_term_cell(
                AnsiColor::Named(NamedColor::Green),
                AnsiColor::Named(NamedColor::Background),
                Flags::INVERSE,
            ),
            context,
        );

        assert!(!inverse_explicit_background.uses_terminal_default_bg);
        assert!((inverse_explicit_background.bg.a - 0.2).abs() <= f32::EPSILON);
    }

    #[test]
    fn pane_neighbor_gaps_prefer_smallest_matching_candidate_gap() {
        let base = tmux_test_pane("%1", 0, 0, 10, 6);
        let far = tmux_test_pane("%2", 15, 0, 3, 6);
        let near = tmux_test_pane("%3", 12, 1, 3, 2);
        let bottom_far = tmux_test_pane("%4", 0, 10, 10, 2);
        let bottom_near = tmux_test_pane("%5", 3, 8, 2, 2);
        let non_overlap = tmux_test_pane("%6", 11, 9, 2, 2);
        let panes = vec![base, far, near, bottom_far, bottom_near, non_overlap];
        let gaps = TerminalView::pane_neighbor_gaps(&panes[0], &panes);
        assert_eq!(gaps.right_cells, Some(2));
        assert_eq!(gaps.bottom_cells, Some(2));
    }

    #[test]
    fn pane_focus_active_border_alpha_is_zero_in_tmux_runtime() {
        let alpha = effective_pane_focus_active_border_alpha(0.38, true, false);
        assert_eq!(alpha, 0.0);
    }

    #[test]
    fn pane_focus_active_border_alpha_is_unchanged_in_native_runtime() {
        let alpha = effective_pane_focus_active_border_alpha(0.38, false, false);
        assert_eq!(alpha, 0.38);
    }

    #[test]
    fn pane_focus_active_border_alpha_is_unchanged_when_tmux_border_is_enabled() {
        let alpha = effective_pane_focus_active_border_alpha(0.38, true, true);
        assert_eq!(alpha, 0.38);
    }

    #[test]
    fn pane_cache_strategy_reuses_cells_when_damage_is_empty_and_key_matches() {
        let strategy = pane_cache_update_strategy(
            true,
            true,
            true,
            true,
            &TerminalDamageSnapshot::Partial(Vec::new()),
        );
        assert_eq!(strategy, PaneCacheUpdateStrategy::Reuse);
    }

    #[test]
    fn pane_cache_strategy_forces_full_rebuild_when_cache_key_changes() {
        let strategy = pane_cache_update_strategy(
            true,
            true,
            true,
            false,
            &TerminalDamageSnapshot::Partial(vec![TerminalDirtySpan {
                row: 0,
                left_col: 0,
                right_col: 1,
            }]),
        );
        assert_eq!(strategy, PaneCacheUpdateStrategy::Full);
    }

    #[test]
    fn pane_cache_strategy_forces_full_rebuild_when_cache_key_changes_with_empty_damage() {
        let strategy = pane_cache_update_strategy(
            true,
            true,
            true,
            false,
            &TerminalDamageSnapshot::Partial(Vec::new()),
        );
        assert_eq!(strategy, PaneCacheUpdateStrategy::Full);
    }

    #[test]
    fn pane_cache_strategy_uses_partial_patch_for_non_empty_partial_damage() {
        let strategy = pane_cache_update_strategy(
            true,
            true,
            true,
            true,
            &TerminalDamageSnapshot::Partial(vec![TerminalDirtySpan {
                row: 1,
                left_col: 2,
                right_col: 4,
            }]),
        );
        assert_eq!(strategy, PaneCacheUpdateStrategy::Partial);
    }

    #[test]
    fn pane_cache_strategy_forces_full_rebuild_when_display_offset_changes() {
        let strategy = pane_cache_update_strategy(
            true,
            true,
            false,
            true,
            &TerminalDamageSnapshot::Partial(vec![TerminalDirtySpan {
                row: 1,
                left_col: 0,
                right_col: 0,
            }]),
        );
        assert_eq!(strategy, PaneCacheUpdateStrategy::Full);
    }

    #[test]
    fn pane_cache_strategy_forces_full_rebuild_when_cache_is_empty() {
        let strategy = pane_cache_update_strategy(
            false,
            true,
            true,
            true,
            &TerminalDamageSnapshot::Partial(vec![TerminalDirtySpan {
                row: 0,
                left_col: 0,
                right_col: 0,
            }]),
        );
        assert_eq!(strategy, PaneCacheUpdateStrategy::Full);
    }

    #[test]
    fn finalized_cache_update_strategy_upgrades_partial_when_fallback_rebuilds() {
        let strategy = finalized_cache_update_strategy(PaneCacheUpdateStrategy::Partial, true);
        assert_eq!(strategy, PaneCacheUpdateStrategy::Full);
    }

    #[test]
    fn finalized_cache_update_strategy_keeps_planned_strategy_without_fallback() {
        assert_eq!(
            finalized_cache_update_strategy(PaneCacheUpdateStrategy::Reuse, false),
            PaneCacheUpdateStrategy::Reuse
        );
        assert_eq!(
            finalized_cache_update_strategy(PaneCacheUpdateStrategy::Partial, false),
            PaneCacheUpdateStrategy::Partial
        );
        assert_eq!(
            finalized_cache_update_strategy(PaneCacheUpdateStrategy::Full, true),
            PaneCacheUpdateStrategy::Full
        );
    }

    #[test]
    fn paint_damage_from_dirty_spans_sorts_and_dedupes_rows() {
        let damage = paint_damage_from_dirty_spans(
            &[
                TerminalDirtySpan {
                    row: 3,
                    left_col: 0,
                    right_col: 1,
                },
                TerminalDirtySpan {
                    row: 1,
                    left_col: 2,
                    right_col: 4,
                },
                TerminalDirtySpan {
                    row: 3,
                    left_col: 5,
                    right_col: 6,
                },
            ],
            4,
        );
        assert_eq!(
            damage,
            TerminalGridPaintDamage::RowRanges(
                vec![(1usize, 2usize, 4usize), (3usize, 0usize, 6usize)].into()
            )
        );
    }

    #[test]
    fn paint_damage_from_dirty_spans_ignores_out_of_bounds_rows() {
        let damage = paint_damage_from_dirty_spans(
            &[TerminalDirtySpan {
                row: 7,
                left_col: 0,
                right_col: 1,
            }],
            2,
        );
        assert_eq!(damage, TerminalGridPaintDamage::None);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn record_cache_strategy_increments_reuse() {
        let mut counts = RenderPassCacheStrategyCounts::default();
        counts.record(PaneCacheUpdateStrategy::Reuse);
        assert_eq!(counts.reuse, 1);
        assert_eq!(counts.partial, 0);
        assert_eq!(counts.full, 0);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn record_cache_strategy_increments_partial() {
        let mut counts = RenderPassCacheStrategyCounts::default();
        counts.record(PaneCacheUpdateStrategy::Partial);
        assert_eq!(counts.reuse, 0);
        assert_eq!(counts.partial, 1);
        assert_eq!(counts.full, 0);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn record_cache_strategy_increments_full() {
        let mut counts = RenderPassCacheStrategyCounts::default();
        counts.record(PaneCacheUpdateStrategy::Full);
        assert_eq!(counts.reuse, 0);
        assert_eq!(counts.partial, 0);
        assert_eq!(counts.full, 1);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn record_partial_work_tracks_dirty_spans_and_patched_cells() {
        let mut counts = RenderPassCacheStrategyCounts::default();
        counts.record_partial_work(3, 12);
        assert_eq!(counts.dirty_span_count, 3);
        assert_eq!(counts.patched_cell_count, 12);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn render_count_increments_once_per_render_call() {
        let mut counters = TerminalRenderMetricsCounters::default();
        increment_render_count_counter(&mut counters);
        assert_eq!(counters.render_count, 1);
    }
}
