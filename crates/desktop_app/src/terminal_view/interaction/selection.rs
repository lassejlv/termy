use super::*;
#[cfg(test)]
use alacritty_terminal::grid::Dimensions;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal_view) struct HoveredLink {
    pub(in crate::terminal_view) start_row: usize,
    pub(in crate::terminal_view) start_col: usize,
    pub(in crate::terminal_view) end_row: usize,
    pub(in crate::terminal_view) end_col: usize,
    pub(in crate::terminal_view) target: String,
}

impl HoveredLink {
    pub(in crate::terminal_view) fn contains(&self, cell: CellPos) -> bool {
        if cell.row < self.start_row || cell.row > self.end_row {
            return false;
        }
        if self.start_row == self.end_row {
            return (self.start_col..=self.end_col).contains(&cell.col);
        }
        if cell.row == self.start_row {
            return cell.col >= self.start_col;
        }
        if cell.row == self.end_row {
            return cell.col <= self.end_col;
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalSelectionCharClass {
    Whitespace,
    Word,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::terminal_view) struct KittyGraphicsPlacementBounds {
    pub(in crate::terminal_view) left: f32,
    pub(in crate::terminal_view) top: f32,
    pub(in crate::terminal_view) width: f32,
    pub(in crate::terminal_view) height: f32,
}

impl KittyGraphicsPlacementBounds {
    fn contains(self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.left + self.width && y >= self.top && y < self.top + self.height
    }
}

pub(in crate::terminal_view) fn kitty_graphics_placement_bounds(
    placement: &KittyGraphicsRenderPlacement,
    cell_width: f32,
    cell_height: f32,
) -> KittyGraphicsPlacementBounds {
    let (width, height) = if placement.virtual_cell.is_some() {
        (cell_width, cell_height)
    } else {
        termy_core::graphics_display_size(
            placement.source_width,
            placement.source_height,
            placement.display_cols,
            placement.display_rows,
            (cell_width, cell_height),
            (placement.x_offset, placement.y_offset),
            false,
        )
    };
    KittyGraphicsPlacementBounds {
        left: (placement.col as f32 + placement.col_offset as f32) * cell_width
            + placement.x_offset as f32,
        top: (placement.viewport_row as f32 + placement.clip_top_rows as f32) * cell_height
            + placement.y_offset as f32,
        width,
        height: (height
            - (placement.clip_top_rows as f32 + placement.clip_bottom_rows as f32) * cell_height)
            .max(0.0),
    }
}

pub(in crate::terminal_view) fn kitty_graphics_placement_intersects_selection(
    placement: &KittyGraphicsRenderPlacement,
    display_offset: usize,
    selection_range: Option<(SelectionPos, SelectionPos)>,
) -> bool {
    let Some((mut start, mut end)) = selection_range else {
        return false;
    };
    if (end.line, end.col) < (start.line, start.col) {
        std::mem::swap(&mut start, &mut end);
    }
    if placement.occupied_cols == 0 || placement.occupied_rows == 0 {
        return false;
    }

    let origin_line = i64::from(placement.viewport_row)
        .saturating_sub(i64::try_from(display_offset).unwrap_or(i64::MAX));
    let image_start_line = origin_line.saturating_add(i64::from(placement.clip_top_rows));
    let image_end_line = origin_line
        .saturating_add(i64::from(placement.occupied_rows))
        .saturating_sub(i64::from(placement.clip_bottom_rows))
        .saturating_sub(1);
    let origin_col = (placement.col as i64).saturating_add(i64::from(placement.col_offset));
    let right_col = origin_col
        .saturating_add(i64::from(placement.occupied_cols))
        .saturating_sub(1);
    if right_col < 0 || image_start_line > image_end_line {
        return false;
    }
    let image_left = origin_col.max(0) as usize;
    let image_right = right_col as usize;
    let start_line = i64::from(start.line);
    let end_line = i64::from(end.line);

    if start_line == end_line {
        return image_start_line <= start_line
            && image_end_line >= start_line
            && image_left <= end.col
            && image_right >= start.col;
    }

    if image_start_line <= start_line && image_end_line >= start_line && image_right >= start.col {
        return true;
    }
    if image_start_line <= end_line && image_end_line >= end_line && image_left <= end.col {
        return true;
    }

    let interior_start = start_line.saturating_add(1);
    let interior_end = end_line.saturating_sub(1);
    interior_start <= interior_end
        && image_start_line <= interior_end
        && image_end_line >= interior_start
}

fn topmost_kitty_graphics_placement_at_point(
    placements: &[KittyGraphicsRenderPlacement],
    x: f32,
    y: f32,
    cell_width: f32,
    cell_height: f32,
    allow_negative_z: bool,
) -> Option<&KittyGraphicsRenderPlacement> {
    placements
        .iter()
        .filter(|placement| allow_negative_z || placement.z_index >= 0)
        .filter(|placement| {
            kitty_graphics_placement_bounds(placement, cell_width, cell_height).contains(x, y)
        })
        .max_by_key(|placement| {
            (
                placement.z_index,
                placement.image_id,
                placement.placement_id,
                placement.placement_serial,
            )
        })
}

fn cell_has_visible_foreground_text(line: &[Option<char>], col: usize) -> bool {
    match line.get(col).copied().flatten() {
        Some(c) => !c.is_whitespace(),
        None if col > 0 => line[col - 1].is_some_and(|c| !c.is_whitespace()),
        None => false,
    }
}

#[cfg(test)]
fn is_hidden_or_spacer(flags: Flags) -> bool {
    flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN)
}

/// Trailing spacer occupies the right half of a wide character on the same
/// line.  Only this variant should trigger the "go back one column" fallback
/// when a selection endpoint or double-click lands on it.
#[cfg(test)]
fn is_trailing_wide_char_spacer(flags: Flags) -> bool {
    flags.contains(Flags::WIDE_CHAR_SPACER) && !flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
}

#[cfg(test)]
fn terminal_line_bounds(
    grid: &alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>,
) -> Option<(i32, i32)> {
    let screen_lines = i32::try_from(grid.screen_lines()).ok()?;
    let total_lines = i32::try_from(grid.total_lines()).ok()?;
    if screen_lines <= 0 || total_lines <= 0 {
        return None;
    }

    let min_line = -(total_lines - screen_lines);
    let max_line = screen_lines - 1;
    Some((min_line, max_line))
}

#[cfg(test)]
fn grid_line_text(
    grid: &alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>,
    line_idx: i32,
    cols: usize,
) -> Option<Vec<Option<char>>> {
    use alacritty_terminal::index::{Column, Line};

    let (min_line, max_line) = terminal_line_bounds(grid)?;
    if line_idx < min_line || line_idx > max_line {
        return None;
    }

    let max_cols = cols.min(grid.columns());
    let mut line = vec![Some(' '); cols];
    let line_ref = &grid[Line(line_idx)];
    for col in 0..max_cols {
        let cell = &line_ref[Column(col)];
        if is_trailing_wide_char_spacer(cell.flags) {
            // Right half of a wide char — mark as None so it is filtered
            // during copy and triggers the col-1 fallback for selection.
            line[col] = None;
            continue;
        }
        if is_hidden_or_spacer(cell.flags) {
            // Leading spacer (wide char wrapped to next line) or hidden
            // cell — keep as space so it does not trigger col-1 fallback.
            continue;
        }

        let c = cell.c;
        if c != '\0' {
            line[col] = Some(if c.is_control() { ' ' } else { c });
        }
    }
    Some(line)
}

type TerminalLineText = Vec<Option<String>>;
type TerminalLineTextRows = Vec<(i32, TerminalLineText)>;

fn full_terminal_selection_range(terminal: &Terminal) -> Option<(SelectionPos, SelectionPos)> {
    // An empty visit reads consistent bounds without traversing the scrollback cells.
    let range = terminal.for_each_line_cell_range(1, 0, |_, _, _, _| {})?;
    if range.columns == 0 || range.first_line > range.last_line {
        return None;
    }
    Some((
        SelectionPos {
            col: 0,
            line: range.first_line,
        },
        SelectionPos {
            col: range.columns - 1,
            line: range.last_line,
        },
    ))
}

fn terminal_line_texts(
    terminal: &Terminal,
    first_line: i32,
    last_line: i32,
) -> Option<(TerminalLineRange, TerminalLineTextRows)> {
    let mut lines = TerminalLineTextRows::new();
    let range = terminal.for_each_line_cell_range(
        first_line,
        last_line,
        |range, line_idx, col, cell| {
            if lines.last().is_none_or(|(line, _)| *line != line_idx) {
                lines.push((line_idx, vec![Some(" ".to_string()); range.columns]));
            }
            let line = &mut lines.last_mut().unwrap().1;
            if col >= line.len() {
                return;
            }
            if cell.is_trailing_wide_spacer() {
                line[col] = None;
                return;
            }
            if cell.is_hidden() || cell.is_wide_spacer() {
                return;
            }
            let character = cell.character();
            if character != '\0' {
                let mut text = String::new();
                text.push(if character.is_control() {
                    ' '
                } else {
                    character
                });
                if !character.is_control() {
                    cell.append_combining_to(&mut text);
                }
                line[col] = Some(text);
            }
        },
    )?;
    Some((range, lines))
}

fn selected_text_from_terminal(
    terminal: &Terminal,
    start: SelectionPos,
    end: SelectionPos,
) -> Option<String> {
    let (raw_start, raw_end) = if (end.line, end.col) < (start.line, start.col) {
        (end, start)
    } else {
        (start, end)
    };
    let (range, captured_lines) = terminal_line_texts(terminal, raw_start.line, raw_end.line)?;
    let cols = range.columns;
    if cols == 0 {
        return None;
    }
    let selection_start = SelectionPos {
        col: raw_start.col.min(cols.saturating_sub(1)),
        line: raw_start.line,
    };
    let selection_end = SelectionPos {
        col: raw_end.col.min(cols.saturating_sub(1)),
        line: raw_end.line,
    };
    let start_line = selection_start.line.max(range.first_line);
    let end_line = selection_end.line.min(range.last_line);
    if start_line > end_line {
        return None;
    }

    let mut lines = Vec::new();
    for (line_idx, line) in captured_lines {
        if line_idx < start_line || line_idx > end_line {
            continue;
        }

        let mut col_start = if line_idx == selection_start.line {
            selection_start.col
        } else {
            0
        };
        let col_end = if line_idx == selection_end.line {
            selection_end.col
        } else {
            cols.saturating_sub(1)
        };
        // If col_start falls on a wide char spacer, move back to the
        // actual character so it is included in the copied text.
        if col_start > 0 && line.get(col_start) == Some(&None) {
            col_start -= 1;
        }
        if col_start > col_end {
            continue;
        }

        let rendered = line[col_start..=col_end]
            .iter()
            .filter_map(|cell| cell.as_deref())
            .collect::<String>()
            .trim_end()
            .to_string();
        lines.push(rendered);
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn row_text_from_terminal(terminal: &Terminal, row: usize, cols: usize) -> Vec<Option<char>> {
    let mut line = vec![Some(' '); cols];
    let _ = terminal.for_each_renderable_cell(|display_offset, term_line, col, cell| {
        let Some(cell_row) = TerminalView::viewport_row_from_term_line(term_line, display_offset)
        else {
            return;
        };
        if cell_row != row || col >= cols {
            return;
        }

        if cell.is_trailing_wide_spacer() {
            line[col] = None;
            return;
        }
        if cell.is_hidden() || cell.is_wide_spacer() {
            return;
        }

        let c = cell.character();
        if c != '\0' {
            line[col] = Some(if c.is_control() { ' ' } else { c });
        }
    });
    line
}

fn line_clickable_end_col(line: &[Option<char>]) -> Option<usize> {
    let last_content_col = line
        .iter()
        .rposition(|cell| cell.is_some_and(|c| !c.is_whitespace()))?;
    Some((last_content_col + 1).min(line.len().saturating_sub(1)))
}

#[allow(clippy::too_many_arguments)]
fn pane_cell_for_position(
    pane: &TerminalPane,
    x: f32,
    y: f32,
    padding_x: f32,
    padding_y: f32,
    layout_cell_width: f32,
    layout_cell_height: f32,
    pane_content_padding_x: f32,
    pane_content_padding_y: f32,
    clamp: bool,
    allow_clamp_outside: bool,
) -> Option<CellPos> {
    let terminal = pane.terminal();
    let size = terminal.size();
    if size.cols == 0 || size.rows == 0 {
        return None;
    }
    let cell_width: f32 = size.cell_width;
    let cell_height: f32 = size.cell_height;
    if cell_width <= f32::EPSILON || cell_height <= f32::EPSILON {
        return None;
    }

    let origin_x = padding_x + (f32::from(pane.left) * layout_cell_width) + pane_content_padding_x;
    let origin_y = padding_y + (f32::from(pane.top) * layout_cell_height) + pane_content_padding_y;
    let width = f32::from(size.cols) * cell_width;
    let height = f32::from(size.rows) * cell_height;
    if width <= f32::EPSILON || height <= f32::EPSILON {
        return None;
    }

    let mut local_x = x - origin_x;
    let mut local_y = y - origin_y;
    let is_inside = local_x >= 0.0 && local_x < width && local_y >= 0.0 && local_y < height;
    if !is_inside {
        if !clamp || !allow_clamp_outside {
            return None;
        }
        local_x = local_x.clamp(0.0, width - f32::EPSILON);
        local_y = local_y.clamp(0.0, height - f32::EPSILON);
    }

    let max_col = i32::from(size.cols) - 1;
    let max_row = i32::from(size.rows) - 1;
    if max_col < 0 || max_row < 0 {
        return None;
    }

    let mut col = (local_x / cell_width).floor() as i32;
    let mut row = (local_y / cell_height).floor() as i32;
    if clamp {
        col = col.clamp(0, max_col);
        row = row.clamp(0, max_row);
    } else if col < 0 || col > max_col || row < 0 || row > max_row {
        return None;
    }

    Some(CellPos {
        col: col as usize,
        row: row as usize,
    })
}

#[allow(clippy::too_many_arguments)]
fn resolve_pane_cell_for_position(
    panes: &[TerminalPane],
    active_pane_id: Option<&str>,
    x: f32,
    y: f32,
    padding_x: f32,
    padding_y: f32,
    layout_cell_width: f32,
    layout_cell_height: f32,
    pane_content_padding_x: f32,
    pane_content_padding_y: f32,
    clamp: bool,
) -> Option<(String, CellPos)> {
    let pointer_inside_any_pane = panes.iter().any(|pane| {
        pane_cell_for_position(
            pane,
            x,
            y,
            padding_x,
            padding_y,
            layout_cell_width,
            layout_cell_height,
            pane_content_padding_x,
            pane_content_padding_y,
            false,
            false,
        )
        .is_some()
    });

    // When clamping points that are outside all panes, prefer the active pane first
    // so we never return a clamped hit for an inactive pane before the active pane.
    if clamp
        && !pointer_inside_any_pane
        && let Some(active_pane_id) = active_pane_id
        && let Some(active_pane) = panes.iter().find(|pane| pane.id.as_str() == active_pane_id)
        && let Some(cell) = pane_cell_for_position(
            active_pane,
            x,
            y,
            padding_x,
            padding_y,
            layout_cell_width,
            layout_cell_height,
            pane_content_padding_x,
            pane_content_padding_y,
            true,
            true,
        )
    {
        return Some((active_pane.id.clone(), cell));
    }

    for pane in panes {
        if let Some(cell) = pane_cell_for_position(
            pane,
            x,
            y,
            padding_x,
            padding_y,
            layout_cell_width,
            layout_cell_height,
            pane_content_padding_x,
            pane_content_padding_y,
            clamp,
            false,
        ) {
            return Some((pane.id.clone(), cell));
        }
    }

    None
}

impl TerminalView {
    fn selection_pos_for_cell_with_display_offset(
        cell: CellPos,
        display_offset: usize,
    ) -> Option<SelectionPos> {
        Some(SelectionPos {
            col: cell.col,
            line: Self::term_line_from_viewport_row(cell.row, display_offset)?,
        })
    }

    pub(in super::super) fn position_to_pane_cell(
        &self,
        position: gpui::Point<Pixels>,
        clamp: bool,
    ) -> Option<(String, CellPos)> {
        let tab = self.session.tabs.get(self.session.active_tab)?;
        let (padding_x, padding_y) = self.effective_terminal_padding();
        let layout_cell_size = self.layout_cell_size();
        let (pane_content_padding_x, pane_content_padding_y) = self.native_split_content_padding();
        let (x, y) = self.terminal_content_position(position);
        resolve_pane_cell_for_position(
            &tab.panes,
            tab.active_pane_id(),
            x,
            y,
            padding_x,
            padding_y,
            layout_cell_size.width.into(),
            layout_cell_size.height.into(),
            pane_content_padding_x,
            pane_content_padding_y,
            clamp,
        )
    }

    fn kitty_image_at_position_with_negative_z(
        &self,
        position: gpui::Point<Pixels>,
        allow_negative_z_over_foreground: bool,
    ) -> Option<KittyImageSelection> {
        let (pane_id, cell) = self.position_to_pane_cell(position, false)?;
        let tab = self.session.tabs.get(self.session.active_tab)?;
        let pane = tab.panes.iter().find(|pane| pane.id == pane_id)?;
        let terminal = pane.terminal();
        let placements = terminal.kitty_graphics_placements();
        if placements.is_empty() {
            return None;
        }
        let size = terminal.size();
        let cell_width = size.cell_width;
        let cell_height = size.cell_height;
        if cell_width <= f32::EPSILON || cell_height <= f32::EPSILON {
            return None;
        }

        let (padding_x, padding_y) = self.effective_terminal_padding();
        let layout_cell_size = self.layout_cell_size();
        let layout_cell_width: f32 = layout_cell_size.width.into();
        let layout_cell_height: f32 = layout_cell_size.height.into();
        let (pane_padding_x, pane_padding_y) = self.native_split_content_padding();
        let (x, y) = self.terminal_content_position(position);
        let local_x = x - padding_x - f32::from(pane.left) * layout_cell_width - pane_padding_x;
        let local_y = y - padding_y - f32::from(pane.top) * layout_cell_height - pane_padding_y;

        let foreground_wins = !allow_negative_z_over_foreground
            && self.pane_cell_has_foreground_semantics(pane_id.as_str(), cell);
        let placement = topmost_kitty_graphics_placement_at_point(
            &placements,
            local_x,
            local_y,
            cell_width,
            cell_height,
            !foreground_wins,
        )?
        .clone();
        Some(KittyImageSelection { pane_id, placement })
    }

    /// Hit-test every visible placement, including graphics behind text. Context menus use
    /// this path so a negative-z image can still offer Copy Image where text overlaps it.
    pub(in super::super) fn kitty_image_at_position(
        &self,
        position: gpui::Point<Pixels>,
    ) -> Option<KittyImageSelection> {
        self.kitty_image_at_position_with_negative_z(position, true)
    }

    /// Left-clicks preserve foreground terminal semantics: text and links rendered above a
    /// negative-z image win the hit, while positive-z images and visible image-only cells remain
    /// selectable.
    pub(in super::super) fn kitty_image_at_position_for_left_click(
        &self,
        position: gpui::Point<Pixels>,
    ) -> Option<KittyImageSelection> {
        self.kitty_image_at_position_with_negative_z(position, false)
    }

    pub(in super::super) fn position_to_cell_in_pane(
        &self,
        pane_id: &str,
        position: gpui::Point<Pixels>,
        clamp: bool,
    ) -> Option<CellPos> {
        let tab = self.session.tabs.get(self.session.active_tab)?;
        let pane = tab.panes.iter().find(|pane| pane.id == pane_id)?;
        let (padding_x, padding_y) = self.effective_terminal_padding();
        let layout_cell_size = self.layout_cell_size();
        let (pane_content_padding_x, pane_content_padding_y) = self.native_split_content_padding();
        let (x, y) = self.terminal_content_position(position);
        pane_cell_for_position(
            pane,
            x,
            y,
            padding_x,
            padding_y,
            layout_cell_size.width.into(),
            layout_cell_size.height.into(),
            pane_content_padding_x,
            pane_content_padding_y,
            clamp,
            true,
        )
    }

    pub(in super::super) fn has_selection(&self) -> bool {
        matches!((self.selection_anchor, self.selection_head), (Some(anchor), Some(head)) if self.selection_moved || anchor != head)
    }

    pub(in super::super) fn selection_range(&self) -> Option<(SelectionPos, SelectionPos)> {
        if !self.has_selection() {
            return None;
        }

        let (anchor, head) = (self.selection_anchor?, self.selection_head?);
        if (head.line, head.col) < (anchor.line, anchor.col) {
            Some((head, anchor))
        } else {
            Some((anchor, head))
        }
    }

    pub(in super::super) fn viewport_row_from_term_line(
        term_line: i32,
        display_offset: usize,
    ) -> Option<usize> {
        let term_line = i64::from(term_line);
        let display_offset = i64::try_from(display_offset).ok()?;
        usize::try_from(term_line + display_offset).ok()
    }

    fn term_line_from_viewport_row(row: usize, display_offset: usize) -> Option<i32> {
        let row = i64::try_from(row).ok()?;
        let display_offset = i64::try_from(display_offset).ok()?;
        i32::try_from(row - display_offset).ok()
    }

    pub(in super::super) fn selection_pos_for_pane_cell(
        &self,
        pane_id: &str,
        cell: CellPos,
    ) -> Option<SelectionPos> {
        let tab = self.session.tabs.get(self.session.active_tab)?;
        let pane = tab.panes.iter().find(|pane| pane.id == pane_id)?;
        let (display_offset, _) = pane.terminal().scroll_state();
        Self::selection_pos_for_cell_with_display_offset(cell, display_offset)
    }

    pub(in super::super) fn pane_cell_has_clickable_text(
        &self,
        pane_id: &str,
        cell: CellPos,
    ) -> bool {
        let Some(tab) = self.session.tabs.get(self.session.active_tab) else {
            return false;
        };
        let Some(pane) = tab.panes.iter().find(|pane| pane.id == pane_id) else {
            return false;
        };
        let terminal = pane.terminal();
        let cols = usize::from(terminal.size().cols);
        if cols == 0 || cell.col >= cols {
            return false;
        }

        let line = row_text_from_terminal(terminal, cell.row, cols);
        line_clickable_end_col(&line).is_some_and(|end_col| cell.col <= end_col)
    }

    fn pane_cell_has_foreground_semantics(&self, pane_id: &str, cell: CellPos) -> bool {
        let Some(terminal) = self.pane_terminal_by_id(pane_id) else {
            return false;
        };
        if terminal.hyperlink_at(cell.row, cell.col).is_some() {
            return true;
        }
        let cols = usize::from(terminal.size().cols);
        if cell.col >= cols {
            return false;
        }
        let line = row_text_from_terminal(terminal, cell.row, cols);
        cell_has_visible_foreground_text(&line, cell.col)
    }

    pub(in super::super) fn selection_pos_for_cell(&self, cell: CellPos) -> Option<SelectionPos> {
        let terminal = self.active_terminal()?;
        let (display_offset, _) = terminal.scroll_state();
        Self::selection_pos_for_cell_with_display_offset(cell, display_offset)
    }

    pub(in super::super) fn position_to_cell(
        &self,
        position: gpui::Point<Pixels>,
        clamp: bool,
    ) -> Option<CellPos> {
        let (pane_id, cell) = self.position_to_pane_cell(position, clamp)?;
        self.is_active_pane_id(&pane_id).then_some(cell)
    }

    pub(in super::super) fn position_to_selection_pos(
        &self,
        position: gpui::Point<Pixels>,
        clamp: bool,
    ) -> Option<SelectionPos> {
        let cell = self.position_to_cell(position, clamp)?;
        self.selection_pos_for_cell(cell)
    }

    pub(in super::super) fn position_to_pane_selection_pos(
        &self,
        position: gpui::Point<Pixels>,
        clamp: bool,
    ) -> Option<(String, SelectionPos)> {
        let (pane_id, cell) = self.position_to_pane_cell(position, clamp)?;
        let selection_pos = self.selection_pos_for_pane_cell(&pane_id, cell)?;
        Some((pane_id, selection_pos))
    }

    /// Sync content_scroll_baseline to the current display_offset after a user-initiated
    /// scroll action, so that the next content-driven change is detected correctly.
    pub(in super::super) fn sync_content_scroll_baseline(&mut self) {
        self.content_scroll_baseline = self
            .active_terminal()
            .map_or(self.content_scroll_baseline, |t| t.scroll_state().0);
    }

    /// Adjust selection positions to compensate for display_offset changes caused by
    /// new terminal content arriving while the user is scrolled into history.
    /// Prevents the selection from visually drifting when Alacritty auto-adjusts the
    /// scroll offset to keep the viewport stable.
    pub(in super::super) fn adjust_selection_for_display_offset_change(
        &mut self,
        old_offset: usize,
        new_offset: usize,
    ) {
        let delta = new_offset as i32 - old_offset as i32;
        if delta == 0 {
            return;
        }
        if let Some(anchor) = &mut self.selection_anchor {
            anchor.line -= delta;
        }
        if let Some(head) = &mut self.selection_head {
            head.line -= delta;
        }
    }

    pub(in super::super) fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        let terminal = self.active_terminal()?;
        selected_text_from_terminal(terminal, start, end)
    }

    pub(in super::super) fn select_all_terminal_contents(&mut self) -> bool {
        let Some((anchor, head)) = self
            .active_terminal()
            .and_then(full_terminal_selection_range)
        else {
            return false;
        };
        self.clear_selection();
        self.pending_cursor_move_click = None;
        self.pending_cursor_move_preview = None;
        self.selection_anchor = Some(anchor);
        self.selection_head = Some(head);
        self.selection_moved = true;
        true
    }

    fn terminal_selection_char_class(c: char) -> TerminalSelectionCharClass {
        if c.is_whitespace() {
            TerminalSelectionCharClass::Whitespace
        } else if c.is_alphanumeric() || c == '_' {
            TerminalSelectionCharClass::Word
        } else {
            TerminalSelectionCharClass::Other
        }
    }

    pub(in super::super) fn select_token_at_cell(&mut self, cell: CellPos) -> bool {
        let Some(terminal) = self.active_terminal() else {
            return false;
        };
        let size = terminal.size();
        let cols = size.cols as usize;
        let rows = size.rows as usize;
        if cols == 0 || cell.row >= rows {
            return false;
        }
        let line = row_text_from_terminal(terminal, cell.row, cols);
        let Some(term_pos) = self.selection_pos_for_cell(cell) else {
            return false;
        };
        if cell.col >= line.len() {
            return false;
        }

        // If the click lands on a wide char spacer, use the actual character.
        let effective_col = if line[cell.col].is_none() && cell.col > 0 {
            cell.col - 1
        } else {
            cell.col
        };
        let Some(effective_char) = line[effective_col] else {
            return false;
        };
        let class = Self::terminal_selection_char_class(effective_char);
        if class == TerminalSelectionCharClass::Whitespace {
            let Some(last_non_whitespace) = line
                .iter()
                .rposition(|c| c.is_some_and(|ch| !ch.is_whitespace()))
            else {
                return false;
            };
            if cell.col > last_non_whitespace {
                return false;
            }
        }

        let mut start_col = effective_col;
        while start_col > 0 {
            match line[start_col - 1] {
                // Spacer: only cross it if the real char beyond it is the same class.
                None if start_col >= 2
                    && line[start_col - 2]
                        .is_some_and(|c| Self::terminal_selection_char_class(c) == class) =>
                {
                    start_col -= 1;
                }
                Some(c) if Self::terminal_selection_char_class(c) == class => start_col -= 1,
                _ => break,
            }
        }

        let mut end_col = effective_col;
        while end_col + 1 < line.len() {
            match line[end_col + 1] {
                // Spacer: only cross it if the real char beyond it is the same class.
                None if end_col + 2 < line.len()
                    && line[end_col + 2]
                        .is_some_and(|c| Self::terminal_selection_char_class(c) == class) =>
                {
                    end_col += 1;
                }
                Some(c) if Self::terminal_selection_char_class(c) == class => end_col += 1,
                _ => break,
            }
        }

        // Include the trailing spacer of the last wide char so the
        // selection highlight covers both cells of the glyph.
        if end_col + 1 < line.len() && line[end_col + 1].is_none() {
            end_col += 1;
        }

        self.selection_anchor = Some(SelectionPos {
            col: start_col,
            line: term_pos.line,
        });
        self.selection_head = Some(SelectionPos {
            col: end_col,
            line: term_pos.line,
        });
        self.selection_dragging = false;
        self.selection_moved = true;
        true
    }

    pub(in super::super) fn select_line_at_row(&mut self, row: usize) -> bool {
        let Some(terminal) = self.active_terminal() else {
            return false;
        };
        let (display_offset, _) = terminal.scroll_state();
        let size = terminal.size();
        let cols = size.cols as usize;
        let rows = size.rows as usize;
        if cols == 0 || row >= rows {
            return false;
        }
        let Some(line) = Self::term_line_from_viewport_row(row, display_offset) else {
            return false;
        };

        self.selection_anchor = Some(SelectionPos { col: 0, line });
        self.selection_head = Some(SelectionPos {
            col: cols.saturating_sub(1),
            line,
        });
        self.selection_dragging = false;
        self.selection_moved = true;
        true
    }

    pub(in super::super) fn link_at_cell(&self, cell: CellPos) -> Option<HoveredLink> {
        let detected = self
            .active_terminal()
            .and_then(|terminal| terminal.link_at(cell.row, cell.col))?;

        Some(HoveredLink {
            start_row: detected.start_row,
            start_col: detected.start_col,
            end_row: detected.end_row,
            end_col: detected.end_col,
            target: detected.target,
        })
    }

    pub(in super::super) fn open_link(url: &str) -> bool {
        if !is_safe_url_for_system_opener(url) {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg(url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok_and(|_| true)
        }
        #[cfg(target_os = "linux")]
        {
            return Command::new("xdg-open")
                .arg(url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map(|_| true)
                .unwrap_or(false);
        }
        #[cfg(target_os = "windows")]
        {
            webbrowser::open(url).is_ok()
        }
    }

    pub(in super::super) fn is_link_modifier(modifiers: gpui::Modifiers) -> bool {
        modifiers.secondary() && !modifiers.alt && !modifiers.function
    }
}

fn is_safe_url_for_system_opener(url: &str) -> bool {
    if url.bytes().any(|byte| byte.is_ascii_control()) {
        return false;
    }

    #[cfg(target_os = "windows")]
    if url.bytes().any(|byte| b"&|<>^".contains(&byte)) {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use termy_core::TerminalSize;

    #[test]
    fn select_all_copies_history_and_viewport_even_while_scrolled() {
        let size = TerminalSize {
            cols: 12,
            rows: 2,
            ..TerminalSize::default()
        };
        for terminal in [
            Terminal::new_test_display(size),
            Terminal::new_tmux(size, TerminalOptions::default()),
        ] {
            terminal.hydrate_output(b"first\r\nsecond\r\nlast");
            let (start, end) = full_terminal_selection_range(&terminal).unwrap();
            assert_eq!(start, SelectionPos { line: -1, col: 0 });
            assert_eq!(end, SelectionPos { line: 1, col: 11 });
            assert_eq!(
                selected_text_from_terminal(&terminal, start, end).as_deref(),
                Some("first\nsecond\nlast")
            );
            assert!(terminal.scroll_display(1));
            assert_eq!(full_terminal_selection_range(&terminal), Some((start, end)));
            assert_eq!(
                selected_text_from_terminal(&terminal, start, end).as_deref(),
                Some("first\nsecond\nlast")
            );
        }
    }

    #[test]
    fn select_all_uses_only_the_active_screen() {
        let size = TerminalSize {
            cols: 12,
            rows: 2,
            ..TerminalSize::default()
        };
        for terminal in [
            Terminal::new_test_display(size),
            Terminal::new_tmux(size, TerminalOptions::default()),
        ] {
            terminal.hydrate_output(b"history\r\nnormal\r\nlast");
            let normal = full_terminal_selection_range(&terminal).unwrap();
            terminal.hydrate_output(b"\x1b[?1049h\x1b[2J\x1b[Htop\r\nbottom");
            let (start, end) = full_terminal_selection_range(&terminal).unwrap();
            assert_eq!(start, SelectionPos { line: 0, col: 0 });
            assert_eq!(end, SelectionPos { line: 1, col: 11 });
            assert_eq!(
                selected_text_from_terminal(&terminal, start, end).as_deref(),
                Some("top\nbottom")
            );
            terminal.hydrate_output(b"\x1b[?1049l");
            assert_eq!(full_terminal_selection_range(&terminal), Some(normal));
            assert_eq!(
                selected_text_from_terminal(&terminal, normal.0, normal.1).as_deref(),
                Some("history\nnormal\nlast")
            );
        }
    }

    #[test]
    fn hovered_link_contains_each_cell_in_wrapped_span() {
        let link = HoveredLink {
            start_row: 2,
            start_col: 4,
            end_row: 4,
            end_col: 2,
            target: "https://example.com".to_string(),
        };

        assert!(!link.contains(CellPos { row: 2, col: 3 }));
        assert!(link.contains(CellPos { row: 2, col: 4 }));
        assert!(link.contains(CellPos { row: 3, col: 0 }));
        assert!(link.contains(CellPos { row: 3, col: 80 }));
        assert!(link.contains(CellPos { row: 4, col: 2 }));
        assert!(!link.contains(CellPos { row: 4, col: 3 }));
    }

    fn kitty_placement(
        placement_serial: u64,
        viewport_row: i32,
        col: usize,
        occupied_cols: u32,
        occupied_rows: u32,
        z_index: i32,
    ) -> KittyGraphicsRenderPlacement {
        KittyGraphicsRenderPlacement {
            placement_serial,
            image_id: 7,
            placement_id: 3,
            image: Arc::new(termy_core::GraphicsImage::from_rgba(
                1,
                1,
                vec![placement_serial as u8; 4],
            )),
            image_width: 20,
            image_height: 40,
            image_generation: 11,
            animation_deadline: None,
            viewport_row,
            col,
            col_offset: 0,
            virtual_cell: None,
            source_x: 0,
            source_y: 0,
            source_width: 20,
            source_height: 40,
            display_cols: Some(2),
            display_rows: Some(2),
            occupied_cols,
            occupied_rows,
            clip_top_rows: 0,
            clip_bottom_rows: 0,
            x_offset: 3,
            y_offset: 4,
            z_index,
        }
    }

    #[test]
    fn kitty_placement_bounds_and_hit_testing_match_render_geometry() {
        let mut earlier = kitty_placement(1, 2, 1, 2, 2, 4);
        let later = kitty_placement(2, 2, 1, 2, 2, 4);
        earlier.image = Arc::new(termy_core::GraphicsImage::from_png(1, 1, vec![99]));
        let placements = vec![earlier, later];

        let bounds = kitty_graphics_placement_bounds(&placements[0], 10.0, 20.0);
        assert_eq!(
            bounds,
            KittyGraphicsPlacementBounds {
                left: 13.0,
                top: 44.0,
                width: 17.0,
                height: 36.0,
            }
        );
        assert_eq!(
            topmost_kitty_graphics_placement_at_point(&placements, 13.0, 44.0, 10.0, 20.0, true,)
                .map(|placement| placement.placement_serial),
            Some(2)
        );
        assert!(
            topmost_kitty_graphics_placement_at_point(&placements, 33.0, 44.0, 10.0, 20.0, true,)
                .is_none()
        );
    }

    #[test]
    fn kitty_hit_testing_prefers_the_highest_z_index_before_identity_order() {
        let mut high_identity_below = kitty_placement(1, 0, 0, 2, 2, -1);
        high_identity_below.image_id = 99;
        let above = kitty_placement(2, 0, 0, 2, 2, 1);

        assert_eq!(
            topmost_kitty_graphics_placement_at_point(
                &[high_identity_below, above],
                4.0,
                5.0,
                10.0,
                20.0,
                true,
            )
            .map(|placement| placement.placement_serial),
            Some(2)
        );
    }

    #[test]
    fn foreground_semantics_only_suppress_negative_z_left_click_hits() {
        let below = kitty_placement(1, 0, 0, 2, 2, -1);
        let above = kitty_placement(2, 0, 0, 2, 2, 1);

        assert!(
            topmost_kitty_graphics_placement_at_point(
                std::slice::from_ref(&below),
                4.0,
                5.0,
                10.0,
                20.0,
                false,
            )
            .is_none()
        );
        assert_eq!(
            topmost_kitty_graphics_placement_at_point(
                std::slice::from_ref(&below),
                4.0,
                5.0,
                10.0,
                20.0,
                true,
            )
            .map(|placement| placement.placement_serial),
            Some(1),
            "right-click policy must retain negative-z image hits"
        );
        assert_eq!(
            topmost_kitty_graphics_placement_at_point(
                &[below, above],
                4.0,
                5.0,
                10.0,
                20.0,
                false,
            )
            .map(|placement| placement.placement_serial),
            Some(2),
            "positive-z images still sit above foreground text"
        );
    }

    #[test]
    fn visible_foreground_text_includes_wide_character_spacers_but_not_blank_cells() {
        let line = vec![Some(' '), Some('桌'), None, Some(' ')];

        assert!(!cell_has_visible_foreground_text(&line, 0));
        assert!(cell_has_visible_foreground_text(&line, 1));
        assert!(cell_has_visible_foreground_text(&line, 2));
        assert!(!cell_has_visible_foreground_text(&line, 3));
    }

    #[test]
    fn kitty_placement_selection_intersection_handles_history_and_linear_ranges() {
        let placement = kitty_placement(1, 3, 4, 3, 3, 0);

        assert!(kitty_graphics_placement_intersects_selection(
            &placement,
            5,
            Some((
                SelectionPos { col: 5, line: -2 },
                SelectionPos { col: 5, line: -2 },
            )),
        ));
        assert!(!kitty_graphics_placement_intersects_selection(
            &placement,
            5,
            Some((
                SelectionPos { col: 0, line: -2 },
                SelectionPos { col: 3, line: -2 },
            )),
        ));
        assert!(kitty_graphics_placement_intersects_selection(
            &placement,
            5,
            Some((
                SelectionPos { col: 70, line: -3 },
                SelectionPos { col: 0, line: -1 },
            )),
        ));
        assert!(kitty_graphics_placement_intersects_selection(
            &placement,
            5,
            Some((
                SelectionPos { col: 6, line: 0 },
                SelectionPos { col: 5, line: -2 },
            )),
        ));
        assert!(!kitty_graphics_placement_intersects_selection(
            &placement,
            5,
            Some((
                SelectionPos { col: 7, line: 1 },
                SelectionPos { col: 9, line: 2 },
            )),
        ));
    }

    #[test]
    fn kitty_image_selection_equality_ignores_png_bytes() {
        let mut first = kitty_placement(9, 0, 0, 1, 1, 0);
        let mut second = first.clone();
        first.image = Arc::new(termy_core::GraphicsImage::from_png(1, 1, vec![1, 2, 3]));
        second.image = Arc::new(termy_core::GraphicsImage::from_png(1, 1, vec![9, 8, 7]));

        assert_eq!(
            KittyImageSelection {
                pane_id: "%1".into(),
                placement: first,
            },
            KittyImageSelection {
                pane_id: "%1".into(),
                placement: second,
            }
        );
    }

    #[test]
    fn kitty_image_selection_resolves_only_an_exact_active_placement() {
        let selected_placement = kitty_placement(9, 0, 0, 1, 1, 0);
        let selection = KittyImageSelection {
            pane_id: "%1".into(),
            placement: selected_placement.clone(),
        };
        let mut current = selected_placement;
        current.image = Arc::new(termy_core::GraphicsImage::from_png(1, 1, vec![99, 98, 97]));

        assert!(
            selection
                .current_placement(Some("%1"), std::slice::from_ref(&current))
                .is_some(),
            "copying resolves the current placement instead of trusting cached PNG bytes"
        );
        assert!(
            selection
                .current_placement(Some("%2"), std::slice::from_ref(&current))
                .is_none(),
            "a selection cannot follow focus to another pane"
        );
        assert!(
            selection.current_placement(Some("%1"), &[]).is_none(),
            "RIS, deletion, and screen switches invalidate placements missing from the active set"
        );

        let identity_mutations: [fn(&mut KittyGraphicsRenderPlacement); 3] = [
            |placement: &mut KittyGraphicsRenderPlacement| placement.placement_serial += 1,
            |placement: &mut KittyGraphicsRenderPlacement| placement.image_id += 1,
            |placement: &mut KittyGraphicsRenderPlacement| placement.image_generation += 1,
        ];
        for mutate_identity in identity_mutations {
            let mut replacement = current.clone();
            mutate_identity(&mut replacement);
            assert!(
                selection
                    .current_placement(Some("%1"), std::slice::from_ref(&replacement))
                    .is_none()
            );
        }
    }

    #[test]
    fn viewport_row_maps_scrollback_lines_into_viewport() {
        assert_eq!(TerminalView::viewport_row_from_term_line(-3, 3), Some(0));
        assert_eq!(TerminalView::viewport_row_from_term_line(4, 3), Some(7));
    }

    #[test]
    fn term_line_round_trips_viewport_row_with_display_offset() {
        let line = TerminalView::term_line_from_viewport_row(0, 4);
        assert_eq!(line, Some(-4));
        assert_eq!(
            TerminalView::viewport_row_from_term_line(line.unwrap(), 4),
            Some(0)
        );
    }

    #[test]
    fn link_modifier_requires_secondary_key() {
        assert!(!TerminalView::is_link_modifier(gpui::Modifiers::default()));
        assert!(TerminalView::is_link_modifier(
            gpui::Modifiers::secondary_key()
        ));
    }

    #[test]
    fn link_modifier_ignores_alt_and_function_chords() {
        assert!(!TerminalView::is_link_modifier(gpui::Modifiers {
            alt: true,
            ..gpui::Modifiers::secondary_key()
        }));
        assert!(!TerminalView::is_link_modifier(gpui::Modifiers {
            function: true,
            ..gpui::Modifiers::secondary_key()
        }));
    }

    #[test]
    fn selection_pos_for_cell_with_display_offset_keeps_col_and_maps_line() {
        assert_eq!(
            TerminalView::selection_pos_for_cell_with_display_offset(CellPos { col: 7, row: 2 }, 4,),
            Some(SelectionPos { col: 7, line: -2 })
        );
    }

    fn non_empty_grid_lines(terminal: &Terminal) -> Vec<(i32, String)> {
        let mut lines = Vec::new();
        let cols = usize::from(terminal.size().cols);
        let _ = terminal.with_tmux_grid(|grid| {
            let Some((min_line, max_line)) = terminal_line_bounds(grid) else {
                return;
            };
            for line_idx in min_line..=max_line {
                let Some(chars) = grid_line_text(grid, line_idx, cols) else {
                    continue;
                };
                let rendered = chars
                    .iter()
                    .filter_map(|c| *c)
                    .collect::<String>()
                    .trim_end()
                    .to_string();
                if !rendered.is_empty() {
                    lines.push((line_idx, rendered));
                }
            }
        });
        lines
    }

    #[test]
    fn hyperlink_at_reports_osc8_link_run() {
        let size = TerminalSize {
            cols: 24,
            rows: 3,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_tmux(size, TerminalOptions::default());
        terminal.feed_output(b"\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\ plain");

        let link = terminal
            .hyperlink_at(0, 2)
            .expect("hovering OSC 8 text should report the hyperlink");
        assert_eq!((link.start_col, link.end_col), (0, 3));
        assert_eq!(link.target, "https://example.com");

        assert_eq!(terminal.hyperlink_at(0, 6), None);
    }

    #[test]
    fn tmon_detects_wrapped_urls_and_osc8_links() {
        let size = TerminalSize {
            cols: 10,
            rows: 4,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_test_display(size);
        terminal.hydrate_output(b"go https://example.com/path");

        for (row, col) in [(0, 4), (1, 5), (2, 4)] {
            let link = terminal
                .link_at(row, col)
                .expect("each wrapped URL segment should resolve");
            assert_eq!((link.start_row, link.start_col), (0, 3));
            assert_eq!((link.end_row, link.end_col), (2, 6));
            assert_eq!(link.target, "https://example.com/path");
        }

        let osc8 = Terminal::new_test_display(TerminalSize {
            cols: 5,
            rows: 3,
            ..TerminalSize::default()
        });
        osc8.hydrate_output(b"\x1b]8;;https://example.com\x1b\\read-more\x1b]8;;\x1b\\");
        let link = osc8
            .link_at(1, 2)
            .expect("wrapped OSC 8 text should resolve as one link");
        assert_eq!((link.start_row, link.start_col), (0, 0));
        assert_eq!((link.end_row, link.end_col), (1, 3));
        assert_eq!(link.target, "https://example.com");
    }

    #[test]
    fn selected_text_from_terminal_stays_stable_after_scrolling_display() {
        let size = TerminalSize {
            cols: 24,
            rows: 3,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 256,
                ..TerminalOptions::default()
            },
        );
        terminal.feed_output(b"line-0\r\nline-1\r\nline-2\r\nline-3\r\nline-4\r\n");

        let lines = non_empty_grid_lines(&terminal);
        let line_1 = lines
            .iter()
            .find(|(_, text)| text.contains("line-1"))
            .map(|(line, _)| *line)
            .expect("line-1 should exist in terminal grid");
        let line_3 = lines
            .iter()
            .find(|(_, text)| text.contains("line-3"))
            .map(|(line, _)| *line)
            .expect("line-3 should exist in terminal grid");

        let start = SelectionPos {
            col: 0,
            line: line_1,
        };
        let end = SelectionPos {
            col: usize::from(size.cols).saturating_sub(1),
            line: line_3,
        };

        let before_scroll =
            selected_text_from_terminal(&terminal, start, end).expect("selection should resolve");
        assert!(before_scroll.contains("line-1"));
        assert!(before_scroll.contains("line-3"));

        assert!(
            terminal.scroll_display(1),
            "expected display offset to change"
        );
        let after_scroll =
            selected_text_from_terminal(&terminal, start, end).expect("selection should resolve");

        assert_eq!(before_scroll, after_scroll);
    }

    #[test]
    fn terminal_read_adapter_extracts_rows_for_all_runtime_variants() {
        let size = TerminalSize {
            cols: 16,
            rows: 3,
            ..TerminalSize::default()
        };

        let tmux = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        tmux.feed_output(b"row-adapter\r\n");
        let tmux_row = row_text_from_terminal(&tmux, 0, usize::from(size.cols));
        assert_eq!(tmux_row.len(), usize::from(size.cols));
        assert!(
            tmux_row
                .iter()
                .any(|c| c.is_some_and(|ch| !ch.is_whitespace()))
        );

        let native = Terminal::new_native(size, None, None, None, None, None, None)
            .expect("native terminal should initialize for row adapter test");
        // Use replay hydration here instead of a live shell command so the row
        // adapter test stays deterministic across different local shells and PTY timing.
        native.hydrate_output(b"native-row-adapter");
        let expected_native_token = "native-row";
        let native_row = row_text_from_terminal(&native, 0, usize::from(size.cols));
        assert_eq!(native_row.len(), usize::from(size.cols));
        let rendered_native_row: String = native_row.iter().filter_map(|c| *c).collect();
        assert!(rendered_native_row.contains(expected_native_token));

        let core_display = Terminal::new_test_display(size);
        core_display.hydrate_output(b"core-row-adapter");
        let core_row = row_text_from_terminal(&core_display, 0, usize::from(size.cols));
        let rendered_core_row: String =
            core_row.iter().filter_map(|character| *character).collect();
        assert!(rendered_core_row.contains("core-row"));
    }

    #[test]
    fn selected_text_reads_core_cells_without_a_tmux_grid() {
        let size = TerminalSize {
            cols: 12,
            rows: 2,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_test_display(size);
        terminal.hydrate_output(b"hello core");

        assert_eq!(
            selected_text_from_terminal(
                &terminal,
                SelectionPos { col: 0, line: 0 },
                SelectionPos { col: 9, line: 0 },
            ),
            Some("hello core".to_string())
        );
    }

    #[test]
    fn selected_text_preserves_core_combining_characters_in_history() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_test_display(size);
        terminal.hydrate_output("e\u{301}\r\nmid\r\nnew".as_bytes());

        assert_eq!(
            selected_text_from_terminal(
                &terminal,
                SelectionPos { col: 0, line: -1 },
                SelectionPos { col: 0, line: -1 },
            ),
            Some("e\u{301}".to_string())
        );
    }

    #[test]
    fn row_text_from_terminal_marks_wide_char_spacers_correctly() {
        let size = TerminalSize {
            cols: 10,
            rows: 3,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        // "ls 文件" — col layout: l s   文 _ 件 _  (where _ = spacer)
        terminal.feed_output("ls 文件".as_bytes());
        let row = row_text_from_terminal(&terminal, 0, usize::from(size.cols));

        assert_eq!(row[0], Some('l'));
        assert_eq!(row[1], Some('s'));
        assert_eq!(row[2], Some(' '));
        assert_eq!(row[3], Some('文'));
        assert_eq!(row[4], None, "trailing spacer should be None");
        assert_eq!(row[5], Some('件'));
        assert_eq!(row[6], None, "trailing spacer should be None");
    }

    #[test]
    fn row_text_from_terminal_leading_spacer_is_not_none() {
        // Narrow terminal forces wide char to wrap, producing a leading spacer.
        let size = TerminalSize {
            cols: 5,
            rows: 3,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        // "src/文" — 4 ASCII chars fill cols 0-3, '文' needs 2 cols but
        // only 1 remains; it wraps to the next line.
        // Line 0 col 4: LEADING_WIDE_CHAR_SPACER
        // Line 1 col 0: '文', col 1: trailing spacer
        terminal.feed_output("src/文".as_bytes());
        let row0 = row_text_from_terminal(&terminal, 0, usize::from(size.cols));
        assert_eq!(
            row0[4],
            Some(' '),
            "leading spacer should be Some(' '), not None"
        );

        let row1 = row_text_from_terminal(&terminal, 1, usize::from(size.cols));
        assert_eq!(row1[0], Some('文'));
        assert_eq!(row1[1], None, "trailing spacer should be None");
    }

    #[test]
    fn line_clickable_end_col_rejects_empty_lines() {
        let line = vec![Some(' '), Some(' '), None, Some(' ')];
        assert_eq!(line_clickable_end_col(&line), None);
    }

    #[test]
    fn line_clickable_end_col_allows_one_cell_after_visible_text() {
        let line = vec![Some('$'), Some(' '), Some('l'), Some('s'), Some(' ')];
        assert_eq!(line_clickable_end_col(&line), Some(4));
    }

    #[test]
    fn line_clickable_end_col_includes_trailing_wide_spacer_cell() {
        let line = vec![Some('好'), None, Some(' ')];
        assert_eq!(line_clickable_end_col(&line), Some(1));
    }

    #[test]
    fn grid_line_text_marks_trailing_wide_char_spacer_as_none() {
        let size = TerminalSize {
            cols: 10,
            rows: 3,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        // "你好" — each char occupies 2 cells: char + trailing spacer.
        terminal.feed_output("你好".as_bytes());

        let line = terminal
            .with_tmux_grid(|grid| grid_line_text(grid, 0, 10))
            .flatten()
            .expect("line 0 should exist");

        assert_eq!(line[0], Some('你'));
        assert_eq!(line[1], None, "trailing spacer should be None");
        assert_eq!(line[2], Some('好'));
        assert_eq!(line[3], None, "trailing spacer should be None");
    }

    #[test]
    fn leading_wide_char_spacer_preserved_as_space() {
        // Use a narrow terminal so a wide char at the end wraps to the next line.
        let size = TerminalSize {
            cols: 5,
            rows: 3,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        // "src/文" — 4 ASCII chars fill cols 0-3, '文' needs 2 cols but
        // only 1 remains; it wraps to the next line.
        // Line 0 col 4: LEADING_WIDE_CHAR_SPACER (should stay Some(' '))
        // Line 1 col 0: '文', col 1: trailing spacer (None)
        terminal.feed_output("src/文".as_bytes());

        let line0 = terminal
            .with_tmux_grid(|grid| grid_line_text(grid, 0, 5))
            .flatten()
            .expect("line 0");
        assert_eq!(
            line0[4],
            Some(' '),
            "leading spacer should be Some(' '), not None"
        );

        let line1 = terminal
            .with_tmux_grid(|grid| grid_line_text(grid, 1, 5))
            .flatten()
            .expect("line 1");
        assert_eq!(line1[0], Some('文'));
        assert_eq!(line1[1], None, "trailing spacer should be None");
    }

    #[test]
    fn selected_text_includes_wide_char_when_start_on_spacer() {
        let size = TerminalSize {
            cols: 10,
            rows: 3,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        // "cd 桌面" layout:
        //   col 0:'c' 1:'d' 2:' ' 3:'桌' 4:spacer 5:'面' 6:spacer
        terminal.feed_output("cd 桌面".as_bytes());

        // Selection starting on the spacer of '桌' (col 4) should fall back to col 3.
        let start = SelectionPos { col: 4, line: 0 };
        let end = SelectionPos { col: 6, line: 0 };
        let text =
            selected_text_from_terminal(&terminal, start, end).expect("selection should resolve");
        assert_eq!(text, "桌面");
    }

    #[test]
    fn selected_text_with_wide_chars_has_no_extra_spaces() {
        let size = TerminalSize {
            cols: 16,
            rows: 3,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        terminal.feed_output("git 提交记录".as_bytes());

        // Select the entire line — copied text should have no extra spaces
        // from wide char spacers.
        let start = SelectionPos { col: 0, line: 0 };
        let end = SelectionPos { col: 15, line: 0 };
        let text =
            selected_text_from_terminal(&terminal, start, end).expect("selection should resolve");
        assert_eq!(text, "git 提交记录");
    }

    #[test]
    fn pane_row_mapping_uses_chrome_adjusted_pointer_y() {
        let chrome_height = 34.0;
        let padding_y = 6.0;
        let pane_top = 1u16;
        let cell_height = 20.0;
        let expected_row = 2i32;

        let window_y = chrome_height
            + padding_y
            + ((f32::from(pane_top) + expected_row as f32) * cell_height)
            + 0.1;
        let content_y = TerminalView::window_y_to_terminal_content_y(window_y, chrome_height);
        let origin_y = padding_y + (f32::from(pane_top) * cell_height);
        let row = ((content_y - origin_y) / cell_height).floor() as i32;

        assert_eq!(row, expected_row);
    }

    #[test]
    fn clamped_pane_lookup_returns_active_pane_when_pointer_inside_active_pane() {
        let rows = 6u16;
        let left_cols = 8u16;
        let right_cols = 8u16;
        let left_terminal = Terminal::new_tmux(
            TerminalSize {
                cols: left_cols,
                rows,
                ..TerminalSize::default()
            },
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        let right_terminal = Terminal::new_tmux(
            TerminalSize {
                cols: right_cols,
                rows,
                ..TerminalSize::default()
            },
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );

        let panes = vec![
            TerminalPane {
                id: "%left".to_string(),
                left: 0,
                top: 0,
                width: left_cols,
                height: rows,
                pane_zoom_steps: 0,
                degraded: false,
                tmux_mouse_mode: None,
                progress_state: ProgressState::default(),
                terminal: left_terminal,
                render_cache: std::cell::RefCell::new(TerminalPaneRenderCache::default()),
                last_alternate_screen: std::cell::Cell::new(false),
                cached_element_ids: PaneCachedElementIds::new("%left"),
            },
            TerminalPane {
                id: "%right".to_string(),
                left: left_cols,
                top: 0,
                width: right_cols,
                height: rows,
                pane_zoom_steps: 0,
                degraded: false,
                tmux_mouse_mode: None,
                progress_state: ProgressState::default(),
                terminal: right_terminal,
                render_cache: std::cell::RefCell::new(TerminalPaneRenderCache::default()),
                last_alternate_screen: std::cell::Cell::new(false),
                cached_element_ids: PaneCachedElementIds::new("%right"),
            },
        ];

        let cell_width: f32 = panes[0].terminal().size().cell_width;
        let cell_height: f32 = panes[0].terminal().size().cell_height;
        let pointer_x = (2.0 * cell_width) + 0.5;
        let pointer_y = (3.0 * cell_height) + 0.5;

        let resolved = resolve_pane_cell_for_position(
            &panes,
            Some("%left"),
            pointer_x,
            pointer_y,
            0.0,
            0.0,
            cell_width,
            cell_height,
            0.0,
            0.0,
            true,
        );
        assert_eq!(
            resolved,
            Some(("%left".to_string(), CellPos { col: 2, row: 3 }))
        );
    }

    #[test]
    fn clamped_pane_lookup_falls_back_to_active_pane_when_pointer_outside_all_panes() {
        let rows = 6u16;
        let left_cols = 8u16;
        let right_cols = 8u16;
        let left_terminal = Terminal::new_tmux(
            TerminalSize {
                cols: left_cols,
                rows,
                ..TerminalSize::default()
            },
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        let right_terminal = Terminal::new_tmux(
            TerminalSize {
                cols: right_cols,
                rows,
                ..TerminalSize::default()
            },
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );

        let panes = vec![
            TerminalPane {
                id: "%left".to_string(),
                left: 0,
                top: 0,
                width: left_cols,
                height: rows,
                pane_zoom_steps: 0,
                degraded: false,
                tmux_mouse_mode: None,
                progress_state: ProgressState::default(),
                terminal: left_terminal,
                render_cache: std::cell::RefCell::new(TerminalPaneRenderCache::default()),
                last_alternate_screen: std::cell::Cell::new(false),
                cached_element_ids: PaneCachedElementIds::new("%left"),
            },
            TerminalPane {
                id: "%right".to_string(),
                left: left_cols,
                top: 0,
                width: right_cols,
                height: rows,
                pane_zoom_steps: 0,
                degraded: false,
                tmux_mouse_mode: None,
                progress_state: ProgressState::default(),
                terminal: right_terminal,
                render_cache: std::cell::RefCell::new(TerminalPaneRenderCache::default()),
                last_alternate_screen: std::cell::Cell::new(false),
                cached_element_ids: PaneCachedElementIds::new("%right"),
            },
        ];

        let active_pane = &panes[0];
        let size = active_pane.terminal().size();
        let cell_width: f32 = size.cell_width;
        let cell_height: f32 = size.cell_height;
        let padding_x = 0.0;
        let padding_y = 0.0;
        let active_origin_x = padding_x + (f32::from(active_pane.left) * cell_width);
        let active_origin_y = padding_y + (f32::from(active_pane.top) * cell_height);
        let active_width = f32::from(active_pane.width) * cell_width;
        let active_height = f32::from(active_pane.height) * cell_height;

        // Outside both panes (to the far right and below) while clamp=true should
        // still return a clamped position in the active pane.
        let pointer_x = active_origin_x + active_width + (f32::from(right_cols) * cell_width);
        let pointer_y = active_origin_y + active_height + (2.0 * cell_height);

        let clamped_x = (pointer_x - active_origin_x).clamp(0.0, active_width - f32::EPSILON);
        let clamped_y = (pointer_y - active_origin_y).clamp(0.0, active_height - f32::EPSILON);
        let expected_col =
            ((clamped_x / cell_width).floor() as i32).clamp(0, i32::from(size.cols) - 1) as usize;
        let expected_row =
            ((clamped_y / cell_height).floor() as i32).clamp(0, i32::from(size.rows) - 1) as usize;

        let resolved = resolve_pane_cell_for_position(
            &panes,
            Some("%left"),
            pointer_x,
            pointer_y,
            padding_x,
            padding_y,
            cell_width,
            cell_height,
            0.0,
            0.0,
            true,
        );
        assert_eq!(
            resolved,
            Some((
                "%left".to_string(),
                CellPos {
                    col: expected_col,
                    row: expected_row,
                },
            ))
        );
    }

    #[test]
    fn file_drop_point_resolves_inactive_pane_without_active_fallback() {
        let cols = 6u16;
        let rows = 4u16;
        let cell_width = 10.0;
        let cell_height = 20.0;
        let make_pane = |id: &str, left: u16| {
            let terminal = Terminal::new_tmux(
                TerminalSize {
                    cols,
                    rows,
                    cell_width,
                    cell_height,
                },
                TerminalOptions {
                    scrollback_history: 128,
                    ..TerminalOptions::default()
                },
            );
            TerminalPane {
                id: id.to_string(),
                left,
                top: 0,
                width: cols,
                height: rows,
                pane_zoom_steps: 0,
                degraded: false,
                tmux_mouse_mode: None,
                progress_state: ProgressState::default(),
                terminal,
                render_cache: std::cell::RefCell::new(TerminalPaneRenderCache::default()),
                last_alternate_screen: std::cell::Cell::new(false),
                cached_element_ids: PaneCachedElementIds::new(id),
            }
        };
        let panes = vec![make_pane("%left", 0), make_pane("%right", cols)];

        let inside_right_x = (f32::from(cols) * cell_width) + 1.0;
        let resolved = resolve_pane_cell_for_position(
            &panes,
            Some("%left"),
            inside_right_x,
            1.0,
            0.0,
            0.0,
            cell_width,
            cell_height,
            0.0,
            0.0,
            false,
        );
        assert_eq!(
            resolved,
            Some(("%right".to_string(), CellPos { col: 0, row: 0 }))
        );

        let outside_x = f32::from(cols * 2) * cell_width + 1.0;
        assert_eq!(
            resolve_pane_cell_for_position(
                &panes,
                Some("%left"),
                outside_x,
                1.0,
                0.0,
                0.0,
                cell_width,
                cell_height,
                0.0,
                0.0,
                false,
            ),
            None
        );
    }

    #[test]
    fn pane_lookup_uses_shared_layout_origin_when_neighbor_has_local_zoom() {
        let rows = 6u16;
        let base_cell_width = 9.0;
        let base_cell_height = 18.0;
        let left_terminal = Terminal::new_tmux(
            TerminalSize {
                cols: 8,
                rows,
                cell_width: base_cell_width,
                cell_height: base_cell_height,
            },
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );
        let right_terminal = Terminal::new_tmux(
            TerminalSize {
                cols: 4,
                rows,
                cell_width: base_cell_width * 2.0,
                cell_height: base_cell_height * 2.0,
            },
            TerminalOptions {
                scrollback_history: 128,
                ..TerminalOptions::default()
            },
        );

        let panes = vec![
            TerminalPane {
                id: "%left".to_string(),
                left: 0,
                top: 0,
                width: 8,
                height: rows,
                pane_zoom_steps: 0,
                degraded: false,
                tmux_mouse_mode: None,
                progress_state: ProgressState::default(),
                terminal: left_terminal,
                render_cache: std::cell::RefCell::new(TerminalPaneRenderCache::default()),
                last_alternate_screen: std::cell::Cell::new(false),
                cached_element_ids: PaneCachedElementIds::new("%left"),
            },
            TerminalPane {
                id: "%right".to_string(),
                left: 8,
                top: 0,
                width: 8,
                height: rows,
                pane_zoom_steps: 2,
                degraded: false,
                tmux_mouse_mode: None,
                progress_state: ProgressState::default(),
                terminal: right_terminal,
                render_cache: std::cell::RefCell::new(TerminalPaneRenderCache::default()),
                last_alternate_screen: std::cell::Cell::new(false),
                cached_element_ids: PaneCachedElementIds::new("%right"),
            },
        ];

        let pointer_x = (8.0 * base_cell_width) + 1.0;
        let pointer_y = 1.0;
        let resolved = resolve_pane_cell_for_position(
            &panes,
            Some("%left"),
            pointer_x,
            pointer_y,
            0.0,
            0.0,
            base_cell_width,
            base_cell_height,
            0.0,
            0.0,
            false,
        );

        assert_eq!(
            resolved,
            Some(("%right".to_string(), CellPos { col: 0, row: 0 }))
        );
    }
}
