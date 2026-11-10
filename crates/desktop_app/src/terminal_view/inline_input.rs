use super::search::SearchEditCommand;
use super::*;
use gpui_kit::{
    App, Bounds, ClipboardItem, ContentMask, ElementInputHandler, Entity, EntityInputHandler, Font,
    Hsla, InputHandler, IntoElement, PaintQuad, Pixels, Point, ShapedLine, TextInputConfiguration,
    TextRun, UTF16Selection, UnderlineStyle, Window, canvas, fill, point, px, size,
};
use std::ops::Range;

use crate::text_input::INLINE_INPUT_LINE_HEIGHT_MULTIPLIER;

fn ime_marked_text_range_utf16(marked_text: Option<&str>) -> Option<Range<usize>> {
    marked_text.map(|marked| 0..marked.encode_utf16().count())
}

fn terminal_ime_selected_text_range(ime_selected_range: Option<Range<usize>>) -> UTF16Selection {
    UTF16Selection {
        range: ime_selected_range.unwrap_or(0..0),
        reversed: false,
    }
}

fn ime_candidate_bounds(
    cursor: Bounds<Pixels>,
    element_bounds: Bounds<Pixels>,
    range_start_x: Pixels,
) -> Bounds<Pixels> {
    let mut bounds = Bounds::new(
        point(
            element_bounds.origin.x + cursor.origin.x,
            element_bounds.origin.y + cursor.origin.y,
        ),
        cursor.size,
    );
    bounds.origin.x += range_start_x;
    bounds
}

/// Byte offset of the first `utf16_len` UTF-16 units of `text`.
fn utf16_prefix_byte_len(text: &str, utf16_len: usize) -> usize {
    let mut units = 0;
    text.char_indices()
        .find_map(|(byte, character)| {
            units += character.len_utf16();
            (units > utf16_len).then_some(byte)
        })
        .unwrap_or(text.len())
}

/// The viewport row showing the cursor at screen row `cursor_row`, kept in
/// the viewport while it is scrolled `display_offset` rows into history.
fn ime_cursor_viewport_row(cursor_row: usize, display_offset: usize, rows: usize) -> usize {
    cursor_row
        .saturating_add(display_offset)
        .min(rows.saturating_sub(1))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InlineInputTarget {
    CommandPalette,
    RenameTab,
    RenameWorkspace,
    Search,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InlineInputNotifyTarget {
    Parent,
    Overlay,
    ParentAndOverlay,
}

pub(super) use crate::text_input::InlineInputState;

pub(super) struct InlineInputElement {
    view: Entity<TerminalView>,
    focus_handle: FocusHandle,
    font: Font,
    font_size: Pixels,
    text_color: Hsla,
    selection_color: Hsla,
    alignment: InlineInputAlignment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InlineInputAlignment {
    Left,
    Center,
}

impl InlineInputElement {
    pub(super) fn new(
        view: Entity<TerminalView>,
        focus_handle: FocusHandle,
        font: Font,
        font_size: Pixels,
        text_color: Hsla,
        selection_color: Hsla,
        alignment: InlineInputAlignment,
    ) -> Self {
        Self {
            view,
            focus_handle,
            font,
            font_size,
            text_color,
            selection_color,
            alignment,
        }
    }
}

pub(super) struct InlineInputPrepaintState {
    lines: Vec<Option<ShapedLine>>,
    line_bounds_vec: Vec<Bounds<Pixels>>,
    line_offset_xs: Vec<Pixels>,
    all_bounds: Bounds<Pixels>,
    selection: Option<PaintQuad>,
    cursor: Option<PaintQuad>,
    cursor_row: usize,
}

impl IntoElement for InlineInputElement {
    type Element = gpui_kit::Canvas<InlineInputPrepaintState>;

    fn into_element(self) -> Self::Element {
        let focus_handle = self.focus_handle;
        let prepaint_focus_handle = focus_handle.clone();
        let view = self.view;
        let prepaint_view = view.clone();
        let font = self.font;
        let font_size = self.font_size;
        let text_color = self.text_color;
        let selection_color = self.selection_color;
        let alignment = self.alignment;

        canvas(
            move |bounds, window, cx| {
                let font_size_value: f32 = font_size.into();
                let line_height =
                    px((font_size_value * INLINE_INPUT_LINE_HEIGHT_MULTIPLIER).round());

                let (
                    text,
                    selected_range,
                    cursor_offset,
                    marked_range,
                    focused,
                    cursor_visible,
                    cursor_style,
                ) = {
                    let view = prepaint_view.read(cx);
                    let focused = prepaint_focus_handle.is_focused(window);
                    let cursor_visible = view.cursor_visible_for_focus(focused);
                    let cursor_style = AppCursorStyle::Line;
                    view.active_inline_input_state().map_or_else(
                        || (String::new(), 0..0, 0, None, focused, false, cursor_style),
                        |state| {
                            (
                                state.text().to_string(),
                                state.selected_range(),
                                state.cursor_offset(),
                                state.marked_range(),
                                focused,
                                cursor_visible,
                                cursor_style,
                            )
                        },
                    )
                };

                // Split text into lines for multiline support
                let raw_lines: Vec<&str> = if text.is_empty() {
                    vec![""]
                } else {
                    text.split('\n').collect()
                };
                let num_lines = raw_lines.len();

                // Compute byte offset for each line start
                let mut line_start_bytes: Vec<usize> = Vec::with_capacity(num_lines);
                {
                    let mut offset = 0usize;
                    for (i, line_str) in raw_lines.iter().enumerate() {
                        line_start_bytes.push(offset);
                        offset += line_str.len();
                        if i + 1 < num_lines {
                            offset += 1; // newline byte
                        }
                    }
                }

                // Determine vertical start: center only for single-line, start from top for multi
                let bounds_height: f32 = bounds.size.height.into();
                let vertical_start = if num_lines == 1 {
                    let extra_height: f32 = (bounds.size.height - line_height).into();
                    bounds.top() + px(extra_height.max(0.0) * 0.5)
                } else {
                    bounds.top()
                };

                // Build per-row bounds
                let mut line_bounds_vec: Vec<Bounds<Pixels>> = Vec::with_capacity(num_lines);
                for i in 0..num_lines {
                    let row_top = vertical_start + line_height * i as f32;
                    // Clamp height so last row doesn't exceed bounds
                    let available = px(bounds_height) - (row_top - bounds.top());
                    let row_height = line_height.min(available.max(px(1.0)));
                    line_bounds_vec.push(Bounds::new(
                        point(bounds.left(), row_top),
                        size(bounds.size.width, row_height),
                    ));
                }

                let cursor_utf8_early = cursor_offset.min(text.len());
                let cursor_row = line_start_bytes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|&(_, start)| *start <= cursor_utf8_early)
                    .map_or(0, |(i, _)| i);
                let cursor_col_in_row = cursor_utf8_early - line_start_bytes[cursor_row];

                let prev_line_offset_xs: Vec<f32> = {
                    let view = prepaint_view.read(cx);
                    view.active_inline_input_state()
                        .map(|s| s.line_offsets().map(f32::from).collect())
                        .unwrap_or_default()
                };

                // Shape each line and compute per-row offset_x
                let mut shaped_lines: Vec<Option<ShapedLine>> = Vec::with_capacity(num_lines);
                let mut line_offset_xs: Vec<Pixels> = Vec::with_capacity(num_lines);

                for (row_idx, line_str) in raw_lines.iter().enumerate() {
                    let shaped = if line_str.is_empty() {
                        None
                    } else {
                        let line_start = line_start_bytes[row_idx];
                        let line_end = line_start + line_str.len();

                        let base_run = TextRun {
                            len: line_str.len(),
                            font: font.clone(),
                            color: text_color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };

                        let runs = if let Some(ref marked_range) = marked_range {
                            // Clip marked range to this line
                            let marked_start_abs = marked_range.start.min(text.len());
                            let marked_end_abs =
                                marked_range.end.min(text.len()).max(marked_start_abs);
                            if marked_end_abs <= line_start || marked_start_abs >= line_end {
                                vec![base_run]
                            } else {
                                let ms = marked_start_abs
                                    .saturating_sub(line_start)
                                    .min(line_str.len());
                                let me = (marked_end_abs - line_start).min(line_str.len());
                                let mut runs = Vec::with_capacity(3);
                                if ms > 0 {
                                    runs.push(TextRun {
                                        len: ms,
                                        ..base_run.clone()
                                    });
                                }
                                if me > ms {
                                    runs.push(TextRun {
                                        len: me - ms,
                                        underline: Some(UnderlineStyle {
                                            color: Some(text_color),
                                            thickness: px(1.0),
                                            wavy: false,
                                        }),
                                        ..base_run.clone()
                                    });
                                }
                                if me < line_str.len() {
                                    runs.push(TextRun {
                                        len: line_str.len() - me,
                                        ..base_run.clone()
                                    });
                                }
                                runs
                            }
                        } else {
                            vec![base_run]
                        };

                        Some(window.text_system().shape_line(
                            (*line_str).to_string().into(),
                            font_size,
                            &runs,
                            None,
                        ))
                    };

                    // Compute offset_x for this row
                    let row_bounds = &line_bounds_vec[row_idx];
                    let offset_x = if row_idx == cursor_row {
                        match alignment {
                            InlineInputAlignment::Left => {
                                let available_width: f32 = row_bounds.size.width.into();
                                let prev_offset: f32 =
                                    prev_line_offset_xs.get(row_idx).copied().unwrap_or(0.0);
                                let cursor_x: f32 = shaped.as_ref().map_or(0.0, |l| -> f32 {
                                    l.x_for_index(cursor_col_in_row).into()
                                });
                                let visible_cursor_x = cursor_x + prev_offset;
                                let padding = 4.0_f32;
                                let new_offset = if visible_cursor_x < 0.0 {
                                    -(cursor_x - padding).max(0.0)
                                } else if visible_cursor_x > available_width - padding {
                                    -(cursor_x - available_width + padding)
                                } else {
                                    prev_offset
                                };
                                px(new_offset.round())
                            }
                            InlineInputAlignment::Center => {
                                let available_width: f32 = row_bounds.size.width.into();
                                let text_width: f32 = shaped.as_ref().map_or(0.0, |l| -> f32 {
                                    l.x_for_index(line_str.len()).into()
                                });
                                if text_width <= available_width {
                                    px(((available_width - text_width) * 0.5).round())
                                } else {
                                    let prev_offset: f32 =
                                        prev_line_offset_xs.get(row_idx).copied().unwrap_or(0.0);
                                    let cursor_x: f32 = shaped.as_ref().map_or(0.0, |l| -> f32 {
                                        l.x_for_index(cursor_col_in_row).into()
                                    });
                                    let visible_cursor_x = cursor_x + prev_offset;
                                    let padding = 4.0_f32;
                                    let new_offset = if visible_cursor_x < 0.0 {
                                        -(cursor_x - padding).max(0.0)
                                    } else if visible_cursor_x > available_width - padding {
                                        -(cursor_x - available_width + padding)
                                    } else {
                                        prev_offset
                                    };
                                    px(new_offset.round())
                                }
                            }
                        }
                    } else {
                        px(0.0)
                    };

                    line_offset_xs.push(offset_x);
                    shaped_lines.push(shaped);
                }

                // all_bounds covers all rows
                let all_bounds = if line_bounds_vec.is_empty() {
                    bounds
                } else {
                    Bounds::from_corners(
                        line_bounds_vec.first().unwrap().origin,
                        point(
                            line_bounds_vec.last().unwrap().right(),
                            line_bounds_vec.last().unwrap().bottom(),
                        ),
                    )
                };

                let selection_start = selected_range.start.min(text.len());
                let selection_end = selected_range.end.min(text.len());

                // Determine selection row extents (only support single-row selection visually)
                let selection = if selection_start < selection_end {
                    // Find which rows the selection spans
                    let sel_start_row = line_start_bytes
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|&(_, s)| *s <= selection_start)
                        .map_or(0, |(i, _)| i);
                    let sel_end_row = line_start_bytes
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|&(_, s)| *s <= selection_end)
                        .map_or(0, |(i, _)| i);

                    if sel_start_row == sel_end_row {
                        let row_bounds = &line_bounds_vec[sel_start_row];
                        let offset_x = line_offset_xs[sel_start_row];
                        let row_start_byte = line_start_bytes[sel_start_row];
                        let start_col = selection_start - row_start_byte;
                        let end_col = selection_end - row_start_byte;
                        let start_x = shaped_lines[sel_start_row]
                            .as_ref()
                            .map_or(px(0.0), |l| l.x_for_index(start_col));
                        let end_x = shaped_lines[sel_start_row]
                            .as_ref()
                            .map_or(px(0.0), |l| l.x_for_index(end_col));
                        Some(fill(
                            Bounds::from_corners(
                                point(row_bounds.left() + offset_x + start_x, row_bounds.top()),
                                point(row_bounds.left() + offset_x + end_x, row_bounds.bottom()),
                            ),
                            selection_color,
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let cursor = if focused && cursor_visible && selection_start == selection_end {
                    let cursor_row_bounds = &line_bounds_vec[cursor_row];
                    let offset_x = line_offset_xs[cursor_row];
                    let cursor_x_in_row = shaped_lines[cursor_row]
                        .as_ref()
                        .map_or(px(0.0), |l| l.x_for_index(cursor_col_in_row));
                    let cursor_x = px({
                        let x: f32 = cursor_x_in_row.into();
                        x.round()
                    });
                    let cursor_width = match cursor_style {
                        AppCursorStyle::Line => px(1.0),
                        AppCursorStyle::Block => {
                            let fallback_width = (font_size_value * 0.62).round().max(1.0);
                            let row_text = raw_lines[cursor_row];
                            let width = row_text
                                .get(cursor_col_in_row..)
                                .and_then(|slice| slice.chars().next())
                                .map(|ch| cursor_col_in_row + ch.len_utf8())
                                .and_then(|next_col| {
                                    shaped_lines[cursor_row]
                                        .as_ref()
                                        .map(|l| l.x_for_index(next_col) - cursor_x_in_row)
                                })
                                .map_or(fallback_width, |delta| {
                                    let w: f32 = delta.into();
                                    w.max(1.0)
                                });
                            px(width)
                        }
                    };
                    let cursor_color = match cursor_style {
                        AppCursorStyle::Line => text_color,
                        AppCursorStyle::Block => selection_color,
                    };
                    Some(fill(
                        Bounds::new(
                            point(
                                cursor_row_bounds.left() + offset_x + cursor_x,
                                cursor_row_bounds.top(),
                            ),
                            size(cursor_width, cursor_row_bounds.size.height),
                        ),
                        cursor_color,
                    ))
                } else {
                    None
                };

                InlineInputPrepaintState {
                    lines: shaped_lines,
                    line_bounds_vec,
                    line_offset_xs,
                    all_bounds,
                    selection,
                    cursor,
                    cursor_row,
                }
            },
            move |bounds, mut prepaint, window, cx| {
                window.handle_input(
                    &focus_handle,
                    ElementInputHandler::new(bounds, view.clone()),
                    cx,
                );

                // Collect layout data before painting (avoid borrow issues)
                let num_lines = prepaint.lines.len();
                let all_bounds = prepaint.all_bounds;

                let painted_lines =
                    window.with_content_mask(Some(ContentMask { bounds: all_bounds }), |window| {
                        if let Some(selection) = prepaint.selection.take() {
                            window.paint_quad(selection);
                        }

                        let mut result_lines: Vec<Option<ShapedLine>> =
                            Vec::with_capacity(num_lines);

                        for i in 0..num_lines {
                            let row_bounds = prepaint.line_bounds_vec[i];
                            let offset_x = prepaint.line_offset_xs[i];
                            let shaped = prepaint.lines[i].take();

                            let painted = if let Some(line) = shaped {
                                line.paint(
                                    point(row_bounds.left() + offset_x, row_bounds.top()),
                                    row_bounds.size.height,
                                    gpui_kit::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                )
                                .expect("failed to paint inline input text");
                                Some(line)
                            } else {
                                None
                            };
                            result_lines.push(painted);
                        }

                        if let Some(cursor) = prepaint.cursor.take() {
                            window.paint_quad(cursor);
                        }

                        result_lines
                    });

                // Build metas for cache
                let line_metas: Vec<(usize, Bounds<Pixels>, Pixels)> = {
                    let mut byte_offset = 0usize;
                    prepaint
                        .line_bounds_vec
                        .iter()
                        .zip(prepaint.line_offset_xs.iter())
                        .zip(painted_lines.iter())
                        .map(|((rb, &ox), pl)| {
                            let start = byte_offset;
                            if let Some(line) = pl {
                                byte_offset += line.len;
                                byte_offset += 1; // newline separator
                            }
                            (start, *rb, ox)
                        })
                        .collect()
                };

                // Use cursor row's data for backward-compat single-line fields
                let cursor_row = prepaint.cursor_row;
                let (cursor_bounds, cursor_offset_x) = line_metas
                    .get(cursor_row)
                    .map_or((all_bounds, px(0.0)), |(_, b, ox)| (*b, *ox));
                let cursor_line = painted_lines.get(cursor_row).cloned().flatten();

                view.update(cx, |this, _cx| {
                    if let Some(state) = this.active_inline_input_state_mut() {
                        state.update_layout_cache(
                            cursor_bounds,
                            cursor_line,
                            cursor_offset_x,
                            line_metas,
                            painted_lines,
                        );
                    }
                });
            },
        )
        .size_full()
    }
}

impl TerminalView {
    fn inline_input_notify_target_for_target(target: InlineInputTarget) -> InlineInputNotifyTarget {
        match target {
            InlineInputTarget::CommandPalette => InlineInputNotifyTarget::Overlay,
            InlineInputTarget::Search => InlineInputNotifyTarget::ParentAndOverlay,
            InlineInputTarget::RenameTab | InlineInputTarget::RenameWorkspace => {
                InlineInputNotifyTarget::Parent
            }
        }
    }

    fn notify_for_inline_input_target(
        &mut self,
        target: InlineInputTarget,
        cx: &mut Context<Self>,
    ) {
        match Self::inline_input_notify_target_for_target(target) {
            InlineInputNotifyTarget::Parent => cx.notify(),
            InlineInputNotifyTarget::Overlay => self.notify_overlay(cx),
            InlineInputNotifyTarget::ParentAndOverlay => {
                self.notify_overlay(cx);
                cx.notify();
            }
        }
    }

    pub(super) fn notify_search_inline_input(&mut self, cx: &mut Context<Self>) {
        self.sync_native_search_bar();
        self.notify_for_inline_input_target(InlineInputTarget::Search, cx);
    }

    /// Whether search owns text input now (nothing drawn over it does).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) fn search_owns_inline_input(&self) -> bool {
        self.active_inline_input_target() == Some(InlineInputTarget::Search)
    }

    fn active_inline_input_target(&self) -> Option<InlineInputTarget> {
        if self.is_command_palette_open() {
            Some(InlineInputTarget::CommandPalette)
        } else if self.search_open {
            Some(InlineInputTarget::Search)
        } else if self.renaming_tab.is_some() {
            Some(InlineInputTarget::RenameTab)
        } else if self.renaming_workspace.is_some() {
            Some(InlineInputTarget::RenameWorkspace)
        } else {
            None
        }
    }

    pub(super) fn has_active_inline_input(&self) -> bool {
        self.active_inline_input_target().is_some()
    }

    pub(super) fn render_inline_input_layer(
        &self,
        font: Font,
        font_size: Pixels,
        text_color: Hsla,
        selection_color: Hsla,
        alignment: InlineInputAlignment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .cursor(gpui_kit::CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(Self::handle_inline_input_mouse_down),
            )
            .on_mouse_move(cx.listener(Self::handle_inline_input_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(Self::handle_inline_input_mouse_up),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(Self::handle_inline_input_mouse_up),
            )
            .child(InlineInputElement::new(
                cx.entity(),
                self.focus_handle.clone(),
                font,
                font_size,
                text_color,
                selection_color,
                alignment,
            ))
            .into_any_element()
    }

    fn active_inline_input_state(&self) -> Option<&InlineInputState> {
        match self.active_inline_input_target()? {
            InlineInputTarget::CommandPalette => Some(self.command_palette_input()),
            InlineInputTarget::Search => Some(&self.search_input),
            InlineInputTarget::RenameTab => Some(&self.rename_input),
            InlineInputTarget::RenameWorkspace => Some(&self.workspace_rename_input),
        }
    }

    fn active_inline_input_state_mut(&mut self) -> Option<&mut InlineInputState> {
        match self.active_inline_input_target()? {
            InlineInputTarget::CommandPalette => Some(self.command_palette_input_mut()),
            InlineInputTarget::Search => Some(&mut self.search_input),
            InlineInputTarget::RenameTab => Some(&mut self.rename_input),
            InlineInputTarget::RenameWorkspace => Some(&mut self.workspace_rename_input),
        }
    }

    pub(super) fn command_palette_query_changed(&mut self, cx: &mut Context<Self>) {
        self.dismiss_command_palette_plugin_ui(cx);
        self.refresh_command_palette_matches(true, cx);
        self.notify_for_inline_input_target(InlineInputTarget::CommandPalette, cx);
    }

    fn enforce_tab_rename_limit(&mut self) {
        let current_chars = self.rename_input.text().chars().count();
        if current_chars <= MAX_TAB_TITLE_CHARS {
            return;
        }

        let truncated: String = self
            .rename_input
            .text()
            .chars()
            .take(MAX_TAB_TITLE_CHARS)
            .collect();
        self.rename_input.set_text(truncated);
    }

    fn enforce_workspace_rename_limit(&mut self) {
        let current_chars = self.workspace_rename_input.text().chars().count();
        if current_chars <= MAX_TAB_TITLE_CHARS {
            return;
        }

        let truncated: String = self
            .workspace_rename_input
            .text()
            .chars()
            .take(MAX_TAB_TITLE_CHARS)
            .collect();
        self.workspace_rename_input.set_text(truncated);
    }

    fn apply_inline_input_mutation(
        &mut self,
        mutate: impl FnOnce(&mut InlineInputState),
        cx: &mut Context<Self>,
    ) {
        self.reset_cursor_blink_phase();

        match self.active_inline_input_target() {
            Some(InlineInputTarget::CommandPalette) => {
                mutate(self.command_palette_input_mut());
                self.command_palette_query_changed(cx);
            }
            Some(InlineInputTarget::Search) => {
                mutate(&mut self.search_input);
                self.handle_search_input_changed(cx);
            }
            Some(InlineInputTarget::RenameTab) => {
                mutate(&mut self.rename_input);
                self.enforce_tab_rename_limit();
                self.notify_for_inline_input_target(InlineInputTarget::RenameTab, cx);
            }
            Some(InlineInputTarget::RenameWorkspace) => {
                mutate(&mut self.workspace_rename_input);
                self.enforce_workspace_rename_limit();
                self.notify_for_inline_input_target(InlineInputTarget::RenameWorkspace, cx);
            }
            None => {}
        }
    }

    pub(super) fn select_all_in_active_inline_input(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.has_active_inline_input() {
            return false;
        }
        self.apply_inline_input_mutation(|state| state.select_all(), cx);
        true
    }

    pub(super) fn paste_clipboard_into_active_inline_input(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.has_active_inline_input() {
            return false;
        }

        let Some(clipboard_text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return true;
        };

        let filtered_text = filter_inline_paste_text(&clipboard_text);
        if filtered_text.is_empty() {
            return true;
        }

        self.apply_inline_input_mutation(
            move |state| state.replace_text_in_range(None, &filtered_text),
            cx,
        );
        true
    }

    pub(super) fn copy_active_inline_input_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(selected_text) = self
            .active_inline_input_state()
            .and_then(|state| state.selected_text())
        else {
            return self.has_active_inline_input();
        };

        cx.write_to_clipboard(ClipboardItem::new_string(selected_text));
        true
    }

    pub(super) fn handle_inline_input_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }

        self.focus_handle.focus(window, cx);

        let target_utf16 = match self.active_inline_input_state() {
            Some(state) => state.character_index_for_point(event.position),
            None => return,
        };

        self.apply_inline_input_mutation(
            |state| {
                if event.modifiers.shift {
                    state.select_to_utf16(target_utf16);
                } else if event.click_count >= 2 {
                    state.select_token_at_utf16(target_utf16);
                } else {
                    state.set_cursor_utf16(target_utf16);
                }
            },
            cx,
        );
        self.inline_input_selecting = true;
        cx.stop_propagation();
    }

    pub(super) fn handle_inline_input_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.inline_input_selecting || !event.dragging() {
            return;
        }

        let target_utf16 = match self.active_inline_input_state() {
            Some(state) => state.character_index_for_point(event.position),
            None => return,
        };

        self.apply_inline_input_mutation(|state| state.select_to_utf16(target_utf16), cx);
        cx.stop_propagation();
    }

    pub(super) fn handle_inline_input_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }

        self.inline_input_selecting = false;
        cx.stop_propagation();
    }

    pub(super) fn handle_inline_backspace_action(
        &mut self,
        _: &crate::commands::InlineBackspace,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::DeleteBackward) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.delete_backward(), cx);
    }

    pub(super) fn handle_inline_delete_action(
        &mut self,
        _: &crate::commands::InlineDelete,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::DeleteForward) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.delete_forward(), cx);
    }

    pub(super) fn handle_inline_move_left_action(
        &mut self,
        _: &crate::commands::InlineMoveLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::MoveLeft) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.move_left(), cx);
    }

    pub(super) fn handle_inline_move_right_action(
        &mut self,
        _: &crate::commands::InlineMoveRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::MoveRight) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.move_right(), cx);
    }

    pub(super) fn handle_inline_select_left_action(
        &mut self,
        _: &crate::commands::InlineSelectLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::SelectLeft) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.select_left(), cx);
    }

    pub(super) fn handle_inline_select_right_action(
        &mut self,
        _: &crate::commands::InlineSelectRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::SelectRight) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.select_right(), cx);
    }

    pub(super) fn handle_inline_select_all_action(
        &mut self,
        _: &crate::commands::InlineSelectAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::SelectAll) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.select_all(), cx);
    }

    pub(super) fn handle_inline_move_to_start_action(
        &mut self,
        _: &crate::commands::InlineMoveToStart,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::MoveToStart) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.move_to_start(), cx);
    }

    pub(super) fn handle_inline_move_to_end_action(
        &mut self,
        _: &crate::commands::InlineMoveToEnd,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::MoveToEnd) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.move_to_end(), cx);
    }

    pub(super) fn handle_inline_delete_word_backward_action(
        &mut self,
        _: &crate::commands::InlineDeleteWordBackward,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::DeleteWordBackward) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.delete_word_backward(), cx);
    }

    pub(super) fn handle_inline_delete_word_forward_action(
        &mut self,
        _: &crate::commands::InlineDeleteWordForward,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::DeleteWordForward) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.delete_word_forward(), cx);
    }

    pub(super) fn handle_inline_delete_to_start_action(
        &mut self,
        _: &crate::commands::InlineDeleteToStart,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::DeleteToStart) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.delete_to_start(), cx);
    }

    pub(super) fn handle_inline_delete_to_end_action(
        &mut self,
        _: &crate::commands::InlineDeleteToEnd,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.forward_edit_to_native_search(SearchEditCommand::DeleteToEnd) {
            return;
        }
        self.apply_inline_input_mutation(|state| state.delete_to_end(), cx);
    }
}

impl TerminalView {
    pub(super) fn ime_cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let geometry = self.terminal_viewport_geometry()?;
        let pane = self.active_pane_ref()?;
        let terminal = pane.terminal();
        let size = terminal.size();
        let cell_width: f32 = size.cell_width;
        let cell_height: f32 = size.cell_height;
        // Use cursor_position() instead of cursor_state() so that IME
        // preedit is shown even when the TUI app hides the cursor.
        let (cursor_col, cursor_row) = terminal.cursor_position();
        let cursor_row = ime_cursor_viewport_row(
            cursor_row,
            terminal.scroll_state().0,
            usize::from(size.rows),
        );
        let x = geometry.origin_x + (cursor_col as f32) * cell_width;
        let y = geometry.origin_y + (cursor_row as f32) * cell_height;
        Some(Bounds::new(
            point(px(x), px(y)),
            gpui_kit::size(px(cell_width), px(cell_height)),
        ))
    }
}

/// Terminal input handler that opts out of macOS press-and-hold, so held keys
/// repeat (e.g. `hjkl` in vim) instead of opening the accent picker.
pub(super) struct TerminalInputHandler(ElementInputHandler<TerminalView>);

impl TerminalInputHandler {
    pub(super) fn new(element_bounds: Bounds<Pixels>, view: Entity<TerminalView>) -> Self {
        Self(ElementInputHandler::new(element_bounds, view))
    }
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.0
            .selected_text_range(ignore_disabled_input, window, cx)
    }

    fn marked_text_range(&mut self, window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.0.marked_text_range(window, cx)
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        self.0
            .text_for_range(range_utf16, adjusted_range, window, cx)
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0
            .replace_text_in_range(replacement_range, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.replace_and_mark_text_in_range(
            range_utf16,
            new_text,
            new_selected_range,
            window,
            cx,
        );
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        self.0.unmark_text(window, cx);
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut App) {
        self.0.paste(item, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.0.bounds_for_range(range_utf16, window, cx)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.0.character_index_for_point(point, window, cx)
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.set_selected_text_range(range_utf16, window, cx);
    }

    fn element_bounds(&mut self, window: &mut Window, cx: &mut App) -> Option<Bounds<Pixels>> {
        self.0.element_bounds(window, cx)
    }

    fn text_length_utf16(&mut self, window: &mut Window, cx: &mut App) -> Option<usize> {
        self.0.text_length_utf16(window, cx)
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }

    fn accepts_text_input(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.0.accepts_text_input(window, cx)
    }

    fn text_input_editable_range(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        self.0.text_input_editable_range(window, cx)
    }

    fn prefers_ime_for_printable_keys(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.0.prefers_ime_for_printable_keys(window, cx)
    }

    fn text_input_configuration(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> TextInputConfiguration {
        self.0.text_input_configuration(window, cx)
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        if let Some(state) = self.active_inline_input_state() {
            return Some(state.text_for_range(range, adjusted_range));
        }
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if let Some(state) = self.active_inline_input_state() {
            return Some(state.selected_text_range());
        }
        Some(terminal_ime_selected_text_range(
            self.ime_selected_range.clone(),
        ))
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        if let Some(state) = self.active_inline_input_state() {
            return state.marked_text_range_utf16();
        }
        ime_marked_text_range_utf16(self.ime_marked_text.as_deref())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.has_active_inline_input() {
            self.apply_inline_input_mutation(|state| state.unmark_text(), cx);
            return;
        }
        // Unmarking IME composition must not commit text to the PTY.
        self.ime_marked_text = None;
        self.ime_selected_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_active_inline_input() {
            self.apply_inline_input_mutation(
                move |state| state.replace_text_in_range(range, text),
                cx,
            );
            return;
        }
        // Terminal IME mode: confirmed text → send to PTY
        let ime_state_changed =
            self.ime_marked_text.take().is_some() || self.ime_selected_range.take().is_some();
        if !text.is_empty() {
            self.write_terminal_input(text.as_bytes(), cx);
        }
        let selection_changed = self.clear_selection();
        if ime_state_changed || selection_changed {
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_active_inline_input() {
            self.apply_inline_input_mutation(
                move |state| {
                    state.replace_and_mark_text_in_range(range, new_text, new_selected_range);
                },
                cx,
            );
            return;
        }
        // Terminal IME mode: store composing text, do NOT send to PTY
        self.ime_marked_text = if new_text.is_empty() {
            None
        } else {
            Some(new_text.to_string())
        };
        self.ime_selected_range = new_selected_range;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if let Some(state) = self.active_inline_input_state() {
            return Some(state.bounds_for_range(range_utf16, element_bounds));
        }
        // ime_cursor_bounds returns coordinates relative to the terminal
        // content area.  Offset by element_bounds.origin to convert to
        // window coordinates so macOS positions the candidate window correctly.
        let cursor = self.ime_cursor_bounds()?;
        // The range counts UTF-16 units of the preedit, which the overlay draws
        // as plain text from the cursor: measure that text, since a CJK glyph
        // spans two cells and an astral character two UTF-16 units.
        let range_start_x = self.ime_marked_text.as_deref().map_or(px(0.0), |marked| {
            let run = TextRun {
                len: marked.len(),
                font: Font {
                    family: self.font_family.clone(),
                    ..gpui_kit::font("")
                },
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(marked.to_string().into(), self.font_size, &[run], None)
                .x_for_index(utf16_prefix_byte_len(marked, range_utf16.start))
        });
        Some(ime_candidate_bounds(cursor, element_bounds, range_start_x))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui_kit::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        if let Some(state) = self.active_inline_input_state() {
            return Some(state.character_index_for_point(point));
        }
        None
    }
}

fn filter_inline_paste_text(text: &str) -> String {
    text.chars()
        .filter(|character| *character != '\n' && *character != '\r')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_range_conversion_handles_multibyte_text() {
        let state = InlineInputState::new("a😄é".to_string());
        let utf16 = state.range_to_utf16(&(1..7));
        assert_eq!(utf16, 1..4);
        let utf8 = state.range_from_utf16(&utf16);
        assert_eq!(utf8, 1..7);
    }

    #[test]
    fn replace_text_uses_selection_when_no_range() {
        let mut state = InlineInputState::new("hello".to_string());
        state.set_cursor_utf16(1);
        state.select_to_utf16(4);
        state.replace_text_in_range(None, "i");
        assert_eq!(state.text(), "hio");
        assert_eq!(state.selected_range(), 2..2);
    }

    #[test]
    fn selected_text_returns_none_without_selection() {
        let state = InlineInputState::new("hello".to_string());

        assert_eq!(state.selected_text(), None);
    }

    #[test]
    fn selected_text_returns_selected_inline_text() {
        let mut state = InlineInputState::new("hello world".to_string());
        state.set_cursor_utf16(0);
        state.select_to_utf16(5);

        assert_eq!(state.selected_text().as_deref(), Some("hello"));
    }

    #[test]
    fn replace_and_mark_sets_marked_and_selection() {
        let mut state = InlineInputState::new("abcd".to_string());
        state.set_cursor_utf16(1);
        state.select_to_utf16(3);
        state.replace_and_mark_text_in_range(Some(1..3), "xy", Some(0..1));
        assert_eq!(state.text(), "axyd");
        assert_eq!(state.marked_range(), Some(1..3));
        assert_eq!(state.selected_range(), 1..2);
    }

    #[test]
    fn unmark_clears_marked_range() {
        let mut state = InlineInputState::new("abc".to_string());
        state.replace_and_mark_text_in_range(Some(0..2), "ab", None);
        state.unmark_text();
        assert_eq!(state.marked_range(), None);
    }

    #[test]
    fn delete_to_start_removes_prefix() {
        let mut state = InlineInputState::new("hello world".to_string());
        state.set_cursor_utf16(5);
        state.delete_to_start();
        assert_eq!(state.text(), " world");
        assert_eq!(state.selected_range(), 0..0);
    }

    #[test]
    fn delete_word_backward_removes_previous_word() {
        let mut state = InlineInputState::new("hello world".to_string());
        state.set_cursor_utf16(11);
        state.delete_word_backward();
        assert_eq!(state.text(), "hello ");
        assert_eq!(state.selected_range(), 6..6);
    }

    #[test]
    fn select_to_utf16_extends_selection() {
        let mut state = InlineInputState::new("a😄b".to_string());
        state.set_cursor_utf16(1);
        state.select_to_utf16(4);
        assert_eq!(state.selected_range(), 1..6);
    }

    #[test]
    fn select_token_at_utf16_selects_word_and_whitespace_runs() {
        let mut state = InlineInputState::new("hello  world".to_string());

        state.select_token_at_utf16(1);
        assert_eq!(state.selected_range(), 0..5);

        state.select_token_at_utf16(5);
        assert_eq!(state.selected_range(), 5..7);
    }

    #[test]
    fn select_token_at_utf16_handles_punctuation_and_end_of_line() {
        let mut state = InlineInputState::new("foo==bar".to_string());

        state.select_token_at_utf16(3);
        assert_eq!(state.selected_range(), 3..5);

        state.select_token_at_utf16(8);
        assert_eq!(state.selected_range(), 5..8);
    }

    #[test]
    fn inline_input_notify_policy_matches_overlay_split() {
        assert_eq!(
            TerminalView::inline_input_notify_target_for_target(InlineInputTarget::CommandPalette),
            InlineInputNotifyTarget::Overlay
        );
        assert_eq!(
            TerminalView::inline_input_notify_target_for_target(InlineInputTarget::Search),
            InlineInputNotifyTarget::ParentAndOverlay
        );
        assert_eq!(
            TerminalView::inline_input_notify_target_for_target(InlineInputTarget::RenameTab),
            InlineInputNotifyTarget::Parent
        );
    }

    #[test]
    fn inline_paste_filter_removes_line_breaks() {
        assert_eq!(
            filter_inline_paste_text("line-1\r\nline-2\nline-3\rline-4"),
            "line-1line-2line-3line-4"
        );
    }

    #[test]
    fn terminal_ime_marked_text_range_counts_utf16_units() {
        assert_eq!(ime_marked_text_range_utf16(None), None);
        assert_eq!(ime_marked_text_range_utf16(Some("a😄")), Some(0..3));
    }

    #[test]
    fn terminal_ime_selected_text_range_defaults_to_caret() {
        let empty = terminal_ime_selected_text_range(None);
        assert_eq!(empty.range, 0..0);
        assert!(!empty.reversed);

        let selected = terminal_ime_selected_text_range(Some(1..4));
        assert_eq!(selected.range, 1..4);
        assert!(!selected.reversed);
    }

    #[test]
    fn ime_candidate_bounds_offsets_cursor_into_window_space() {
        let cursor = Bounds::new(point(px(10.0), px(20.0)), size(px(8.0), px(16.0)));
        let element_bounds = Bounds::new(point(px(100.0), px(200.0)), size(px(320.0), px(240.0)));

        let bounds = ime_candidate_bounds(cursor, element_bounds, px(16.0));

        assert_eq!(bounds.origin, point(px(126.0), px(220.0)));
        assert_eq!(bounds.size, cursor.size);
    }

    #[test]
    fn ime_range_offsets_map_utf16_units_to_preedit_bytes() {
        assert_eq!(utf16_prefix_byte_len("ab", 1), 1);
        // Each CJK character is one UTF-16 unit but three bytes.
        assert_eq!(utf16_prefix_byte_len("你好", 1), 3);
        // An astral character is two UTF-16 units and four bytes.
        assert_eq!(utf16_prefix_byte_len("\u{20000}x", 2), 4);
        assert_eq!(utf16_prefix_byte_len("你好", 9), "你好".len());
    }

    #[test]
    fn ime_cursor_row_follows_the_cursor_while_scrolled_into_history() {
        assert_eq!(ime_cursor_viewport_row(3, 0, 24), 3);
        assert_eq!(ime_cursor_viewport_row(3, 5, 24), 8);
        assert_eq!(ime_cursor_viewport_row(20, 10, 24), 23);
    }
}
