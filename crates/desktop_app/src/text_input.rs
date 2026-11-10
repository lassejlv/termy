use crate::text_editing;
use gpui_kit::{
    Bounds, ElementInputHandler, Entity, EntityInputHandler, Font, Hsla, IntoElement, PaintQuad,
    Pixels, ShapedLine, Styled, TextRun, UTF16Selection, UnderlineStyle, canvas, fill, point, px,
    size,
};
use std::ops::Range;

mod inline;
mod multiline;

pub(crate) use inline::InlineInputState;

pub use multiline::MultilineTextInputElement;

pub(crate) const INLINE_INPUT_LINE_HEIGHT_MULTIPLIER: f32 = 1.35;

/// Shared text input state for single-line and multiline text fields.
#[derive(Clone, Debug)]
pub struct TextInputState {
    text: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    last_line_offset_x: Pixels,
    last_line_metas: Vec<(usize, Bounds<Pixels>, Pixels)>,
    last_line_layouts: Vec<Option<ShapedLine>>,
}

impl TextInputState {
    pub fn new(text: String) -> Self {
        let mut state = Self {
            text,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            last_line_offset_x: px(0.0),
            last_line_metas: Vec::new(),
            last_line_layouts: Vec::new(),
        };
        state.move_to_end();
        state
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.marked_range = None;
        self.selection_reversed = false;
        self.invalidate_layout();
        self.move_to_end();
    }

    pub(crate) fn clear(&mut self) {
        self.set_text(String::new());
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        (!self.selected_range.is_empty())
            .then(|| self.text[self.selected_range.clone()].to_string())
    }

    pub fn move_to_end(&mut self) {
        self.set_cursor_utf8(self.text.len());
    }

    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub fn selected_range(&self) -> Range<usize> {
        self.selected_range.clone()
    }

    pub fn select_all(&mut self) {
        self.selection_reversed = false;
        self.selected_range = 0..self.text.len();
    }

    pub fn marked_range(&self) -> Option<Range<usize>> {
        self.marked_range.clone()
    }

    fn set_cursor_utf8(&mut self, offset: usize) {
        let offset = text_editing::clamp_utf8_index(&self.text, offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.marked_range = None;
    }

    pub fn select_to_utf8(&mut self, offset: usize) {
        let offset = text_editing::clamp_utf8_index(&self.text, offset);
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.marked_range = None;
    }

    pub fn set_cursor_utf16(&mut self, offset: usize) {
        let utf8_offset = text_editing::utf16_to_utf8(&self.text, offset);
        self.set_cursor_utf8(utf8_offset);
    }

    pub fn select_to_utf16(&mut self, offset: usize) {
        let utf8_offset = text_editing::utf16_to_utf8(&self.text, offset);
        self.select_to_utf8(utf8_offset);
    }

    fn previous_char_boundary(&self, offset: usize) -> usize {
        text_editing::previous_char_boundary(&self.text, offset)
    }

    fn next_char_boundary(&self, offset: usize) -> usize {
        text_editing::next_char_boundary(&self.text, offset)
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        text_editing::previous_word_boundary(&self.text, offset)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        text_editing::next_word_boundary(&self.text, offset)
    }

    fn select_range_utf8(&mut self, range: Range<usize>) {
        self.selected_range = text_editing::clamped_utf8_range(&self.text, range);
        self.selection_reversed = false;
        self.marked_range = None;
    }

    fn token_range_at_utf8(&self, offset: usize) -> Range<usize> {
        text_editing::token_range_at_utf8(&self.text, offset)
    }

    pub fn select_token_at_utf16(&mut self, offset_utf16: usize) {
        let utf8_offset = text_editing::utf16_to_utf8(&self.text, offset_utf16);
        let range = self.token_range_at_utf8(utf8_offset);
        self.select_range_utf8(range);
    }

    fn delete_range_utf8(&mut self, range: Range<usize>) {
        if range.start >= range.end || range.end > self.text.len() {
            return;
        }
        self.text.replace_range(range.clone(), "");
        self.set_cursor_utf8(range.start);
        self.invalidate_layout();
    }

    fn delete_selected_or(&mut self, fallback: Range<usize>) {
        let range = if self.selected_range.is_empty() {
            fallback
        } else {
            self.selected_range.clone()
        };
        self.delete_range_utf8(range);
    }

    pub fn move_left(&mut self) {
        if !self.selected_range.is_empty() {
            self.set_cursor_utf8(self.selected_range.start);
            return;
        }
        let cursor = self.cursor_offset();
        self.set_cursor_utf8(self.previous_char_boundary(cursor));
    }

    pub fn move_right(&mut self) {
        if !self.selected_range.is_empty() {
            self.set_cursor_utf8(self.selected_range.end);
            return;
        }
        let cursor = self.cursor_offset();
        self.set_cursor_utf8(self.next_char_boundary(cursor));
    }

    fn vertical_offset(&self, direction: isize) -> Option<usize> {
        let cursor = self.cursor_offset();
        let line_start = self.text[..cursor].rfind('\n').map_or(0, |index| index + 1);
        let column = self.text[line_start..cursor].chars().count();
        if direction < 0 {
            let previous_end = line_start.checked_sub(1)?;
            let previous_start = self.text[..previous_end]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            Some(
                previous_start
                    + self.text[previous_start..previous_end]
                        .chars()
                        .take(column)
                        .map(char::len_utf8)
                        .sum::<usize>(),
            )
        } else {
            let current_end = self.text[cursor..].find('\n').map(|index| cursor + index)?;
            let next_start = current_end + 1;
            let next_end = self.text[next_start..]
                .find('\n')
                .map_or(self.text.len(), |index| next_start + index);
            Some(
                next_start
                    + self.text[next_start..next_end]
                        .chars()
                        .take(column)
                        .map(char::len_utf8)
                        .sum::<usize>(),
            )
        }
    }

    pub fn move_up(&mut self) {
        if !self.selected_range.is_empty() {
            self.set_cursor_utf8(self.selected_range.start);
        } else if let Some(offset) = self.vertical_offset(-1) {
            self.set_cursor_utf8(offset);
        }
    }

    pub fn move_down(&mut self) {
        if !self.selected_range.is_empty() {
            self.set_cursor_utf8(self.selected_range.end);
        } else if let Some(offset) = self.vertical_offset(1) {
            self.set_cursor_utf8(offset);
        }
    }

    pub fn select_up(&mut self) {
        if let Some(offset) = self.vertical_offset(-1) {
            self.select_to_utf8(offset);
        }
    }

    pub fn select_down(&mut self) {
        if let Some(offset) = self.vertical_offset(1) {
            self.select_to_utf8(offset);
        }
    }

    pub fn select_left(&mut self) {
        let cursor = self.cursor_offset();
        self.select_to_utf8(self.previous_char_boundary(cursor));
    }

    pub fn select_right(&mut self) {
        let cursor = self.cursor_offset();
        self.select_to_utf8(self.next_char_boundary(cursor));
    }

    pub fn move_to_start(&mut self) {
        self.set_cursor_utf8(0);
    }

    pub fn delete_backward(&mut self) {
        let cursor = self.cursor_offset();
        let start = self.previous_char_boundary(cursor);
        self.delete_selected_or(start..cursor);
    }

    pub fn delete_forward(&mut self) {
        let cursor = self.cursor_offset();
        let end = self.next_char_boundary(cursor);
        self.delete_selected_or(cursor..end);
    }

    pub fn delete_word_backward(&mut self) {
        let cursor = self.cursor_offset();
        let start = self.previous_word_boundary(cursor);
        self.delete_selected_or(start..cursor);
    }

    pub fn delete_word_forward(&mut self) {
        let cursor = self.cursor_offset();
        let end = self.next_word_boundary(cursor);
        self.delete_selected_or(cursor..end);
    }

    pub fn delete_to_start(&mut self) {
        let cursor = self.cursor_offset();
        self.delete_selected_or(0..cursor);
    }

    pub fn delete_to_end(&mut self) {
        let cursor = self.cursor_offset();
        self.delete_selected_or(cursor..self.text.len());
    }

    fn invalidate_layout(&mut self) {
        self.last_layout = None;
        self.last_line_metas.clear();
        self.last_line_layouts.clear();
    }

    pub fn update_layout_cache(
        &mut self,
        bounds: Bounds<Pixels>,
        layout: Option<ShapedLine>,
        line_offset_x: Pixels,
    ) {
        self.last_bounds = Some(bounds);
        self.last_layout = layout;
        self.last_line_offset_x = line_offset_x;
        self.last_line_metas.clear();
        self.last_line_layouts.clear();
    }

    fn update_multiline_layout_cache(
        &mut self,
        bounds: Bounds<Pixels>,
        line_metas: Vec<(usize, Bounds<Pixels>, Pixels)>,
        line_layouts: Vec<Option<ShapedLine>>,
    ) {
        self.last_bounds = Some(bounds);
        self.last_layout = None;
        self.last_line_offset_x = px(0.0);
        self.last_line_metas = line_metas;
        self.last_line_layouts = line_layouts;
    }

    pub fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        text_editing::range_from_utf16(&self.text, range_utf16)
    }

    pub fn range_to_utf16(&self, range_utf8: &Range<usize>) -> Range<usize> {
        text_editing::range_to_utf16(&self.text, range_utf8)
    }

    pub fn utf8_to_utf16(&self, utf8_offset: usize) -> usize {
        text_editing::utf8_to_utf16(&self.text, utf8_offset)
    }

    fn replacement_range(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range())
    }

    pub fn text_for_range(
        &self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
    ) -> String {
        let range = self.range_from_utf16(&range_utf16);
        adjusted_range.replace(self.range_to_utf16(&range));
        self.text[range].to_string()
    }

    pub fn bounds_for_range(
        &self,
        range_utf16: Range<usize>,
        fallback_bounds: Bounds<Pixels>,
    ) -> Bounds<Pixels> {
        let range = self.range_from_utf16(&range_utf16);
        if let Some(index) = self
            .last_line_metas
            .iter()
            .rposition(|(start, _, _)| *start <= range.start)
        {
            let (line_start, bounds, offset_x) = self.last_line_metas[index];
            let layout = self.last_line_layouts.get(index).and_then(Option::as_ref);
            let line_len = layout.map_or(0, |line| line.len);
            let start = range.start.saturating_sub(line_start).min(line_len);
            let end = range.end.saturating_sub(line_start).min(line_len);
            let start_x = layout.map_or(px(0.0), |line| line.x_for_index(start));
            let end_x = layout.map_or(start_x, |line| line.x_for_index(end));
            return Bounds::from_corners(
                point(bounds.left() + offset_x + start_x, bounds.top()),
                point(bounds.left() + offset_x + end_x, bounds.bottom()),
            );
        }

        let bounds = self.last_bounds.unwrap_or(fallback_bounds);
        let (start_x, end_x) = if let Some(layout) = self.last_layout.as_ref() {
            (
                layout.x_for_index(range.start),
                layout.x_for_index(range.end),
            )
        } else {
            (px(0.0), px(0.0))
        };

        Bounds::from_corners(
            point(
                bounds.left() + self.last_line_offset_x + start_x,
                bounds.top(),
            ),
            point(
                bounds.left() + self.last_line_offset_x + end_x,
                bounds.bottom(),
            ),
        )
    }

    pub fn selected_text_range(&self) -> UTF16Selection {
        UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        }
    }

    pub fn marked_text_range_utf16(&self) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    pub fn character_index_for_point(&self, point: gpui_kit::Point<Pixels>) -> usize {
        if self.text.is_empty() {
            return 0;
        }

        if !self.last_line_metas.is_empty() {
            let index = self
                .last_line_metas
                .iter()
                .position(|(_, bounds, _)| point.y <= bounds.bottom())
                .unwrap_or(self.last_line_metas.len() - 1);
            let (line_start, bounds, offset_x) = self.last_line_metas[index];
            let local_x = (point.x - bounds.left() - offset_x).max(px(0.0));
            let local_index = self.last_line_layouts[index]
                .as_ref()
                .map_or(0, |line| line.closest_index_for_x(local_x));
            return self.utf8_to_utf16(line_start + local_index);
        }

        let Some(bounds) = self.last_bounds else {
            return self.range_to_utf16(&self.selected_range).start;
        };

        if point.y < bounds.top() {
            return 0;
        }
        if point.y > bounds.bottom() {
            return self.utf8_to_utf16(self.text.len());
        }

        let text_left = bounds.left() + self.last_line_offset_x;
        let local_x = if point.x <= text_left {
            px(0.0)
        } else {
            point.x - text_left
        };

        let utf8_index = self
            .last_layout
            .as_ref()
            .map_or(0, |layout| layout.closest_index_for_x(local_x));
        self.utf8_to_utf16(utf8_index)
    }

    pub fn unmark_text(&mut self) {
        self.marked_range = None;
    }

    pub fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, text: &str) {
        let range = self.replacement_range(range_utf16);
        self.text.replace_range(range.clone(), text);
        let cursor = range.start + text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        self.invalidate_layout();
    }

    pub fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) {
        let range = self.replacement_range(range_utf16);
        self.text.replace_range(range.clone(), new_text);

        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }

        self.selection_reversed = false;
        if let Some(local_selected_utf16) = new_selected_range_utf16 {
            let local_selected = text_editing::range_from_utf16(new_text, &local_selected_utf16);
            let selected_start = range.start + local_selected.start;
            let selected_end = range.start + local_selected.end;
            self.selected_range = selected_start..selected_end;
        } else {
            let cursor = range.start + new_text.len();
            self.selected_range = cursor..cursor;
        }
        self.invalidate_layout();
    }
}

/// Trait for views that can provide text input state for the TextInputElement.
pub trait TextInputProvider: 'static + Sized {
    /// Returns the current text input state, if any.
    fn text_input_state(&self) -> Option<&TextInputState>;

    /// Returns a mutable reference to the current text input state, if any.
    fn text_input_state_mut(&mut self) -> Option<&mut TextInputState>;

    /// Returns whether the cursor should be visible. Defaults to true.
    fn cursor_visible(&self) -> bool {
        true
    }

    /// Returns the cursor style (line width). Defaults to 1.0px (line cursor).
    fn cursor_width(&self, _font_size: f32) -> f32 {
        1.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextInputAlignment {
    Left,
}

pub struct TextInputPrepaintState {
    line: Option<ShapedLine>,
    line_bounds: Bounds<Pixels>,
    line_offset_x: Pixels,
    selection: Option<PaintQuad>,
    cursor: Option<PaintQuad>,
}

pub struct TextInputElement<V: TextInputProvider> {
    view: Entity<V>,
    focus_handle: gpui_kit::FocusHandle,
    font: Font,
    font_size: Pixels,
    text_color: Hsla,
    selection_color: Hsla,
    alignment: TextInputAlignment,
}

impl<V: TextInputProvider> TextInputElement<V> {
    pub fn new(
        view: Entity<V>,
        focus_handle: gpui_kit::FocusHandle,
        font: Font,
        font_size: Pixels,
        text_color: Hsla,
        selection_color: Hsla,
        alignment: TextInputAlignment,
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

impl<V: TextInputProvider + gpui_kit::Render + EntityInputHandler> IntoElement
    for TextInputElement<V>
{
    type Element = gpui_kit::Canvas<TextInputPrepaintState>;

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
                let bounds_height: f32 = bounds.size.height.into();
                let target_line_height = (font_size_value * INLINE_INPUT_LINE_HEIGHT_MULTIPLIER)
                    .round()
                    .clamp(1.0, bounds_height.max(1.0));
                let line_height = px(target_line_height);
                let extra_height: f32 = (bounds.size.height - line_height).into();
                let vertical_offset = px(extra_height.max(0.0) * 0.5);
                let line_bounds = Bounds::new(
                    point(bounds.left(), bounds.top() + vertical_offset),
                    size(bounds.size.width, line_height),
                );

                let (
                    text,
                    selected_range,
                    cursor_offset,
                    marked_range,
                    focused,
                    cursor_visible,
                    cursor_width,
                ) = {
                    let view = prepaint_view.read(cx);
                    let focused = prepaint_focus_handle.is_focused(window);
                    let cursor_visible = view.cursor_visible();
                    let cursor_width = view.cursor_width(font_size_value);
                    view.text_input_state().map_or_else(
                        || (String::new(), 0..0, 0, None, focused, false, cursor_width),
                        |state| {
                            (
                                state.text().to_string(),
                                state.selected_range(),
                                state.cursor_offset(),
                                state.marked_range.clone(),
                                focused,
                                cursor_visible,
                                cursor_width,
                            )
                        },
                    )
                };

                let line = if text.is_empty() {
                    None
                } else {
                    let base_run = TextRun {
                        len: text.len(),
                        font: font.clone(),
                        color: text_color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };

                    let runs = if let Some(marked_range) = marked_range {
                        let marked_start = marked_range.start.min(text.len());
                        let marked_end = marked_range.end.min(text.len()).max(marked_start);
                        let mut runs = Vec::with_capacity(3);
                        if marked_start > 0 {
                            runs.push(TextRun {
                                len: marked_start,
                                ..base_run.clone()
                            });
                        }
                        if marked_end > marked_start {
                            runs.push(TextRun {
                                len: marked_end - marked_start,
                                underline: Some(UnderlineStyle {
                                    color: Some(text_color),
                                    thickness: px(1.0),
                                    wavy: false,
                                }),
                                ..base_run.clone()
                            });
                        }
                        if marked_end < text.len() {
                            runs.push(TextRun {
                                len: text.len() - marked_end,
                                ..base_run
                            });
                        }
                        runs
                    } else {
                        vec![base_run]
                    };

                    Some(window.text_system().shape_line(
                        text.clone().into(),
                        font_size,
                        &runs,
                        None,
                    ))
                };

                let line_offset_x = match alignment {
                    TextInputAlignment::Left => px(0.0),
                };

                let cursor_utf8 = cursor_offset.min(text.len());
                let selection_start = selected_range.start.min(text.len());
                let selection_end = selected_range.end.min(text.len());

                let selection = if selection_start < selection_end {
                    let start_x = line
                        .as_ref()
                        .map_or(px(0.0), |line| line.x_for_index(selection_start));
                    let end_x = line
                        .as_ref()
                        .map_or(px(0.0), |line| line.x_for_index(selection_end));
                    Some(fill(
                        Bounds::from_corners(
                            point(
                                line_bounds.left() + line_offset_x + start_x,
                                line_bounds.top(),
                            ),
                            point(
                                line_bounds.left() + line_offset_x + end_x,
                                line_bounds.bottom(),
                            ),
                        ),
                        selection_color,
                    ))
                } else {
                    None
                };

                let cursor = if focused && cursor_visible && selection_start == selection_end {
                    let cursor_x = line
                        .as_ref()
                        .map_or(px(0.0), |line| line.x_for_index(cursor_utf8));
                    let cursor_x = px({
                        let x: f32 = cursor_x.into();
                        x.round()
                    });

                    Some(fill(
                        Bounds::new(
                            point(
                                line_bounds.left() + line_offset_x + cursor_x,
                                line_bounds.top(),
                            ),
                            size(px(cursor_width), line_bounds.size.height),
                        ),
                        text_color,
                    ))
                } else {
                    None
                };

                TextInputPrepaintState {
                    line,
                    line_bounds,
                    line_offset_x,
                    selection,
                    cursor,
                }
            },
            move |bounds, mut prepaint, window, cx| {
                window.handle_input(
                    &focus_handle,
                    ElementInputHandler::new(bounds, view.clone()),
                    cx,
                );

                if let Some(selection) = prepaint.selection.take() {
                    window.paint_quad(selection);
                }

                let line = if let Some(line) = prepaint.line.take() {
                    line.paint(
                        point(
                            prepaint.line_bounds.left() + prepaint.line_offset_x,
                            prepaint.line_bounds.top(),
                        ),
                        prepaint.line_bounds.size.height,
                        gpui_kit::TextAlign::Left,
                        None,
                        window,
                        cx,
                    )
                    .expect("failed to paint text input text");
                    Some(line)
                } else {
                    None
                };

                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }

                view.update(cx, |this, _cx| {
                    if let Some(state) = this.text_input_state_mut() {
                        state.update_layout_cache(
                            prepaint.line_bounds,
                            line,
                            prepaint.line_offset_x,
                        );
                    }
                });
            },
        )
        .size_full()
    }
}

/// Helper macro to implement EntityInputHandler for a type using TextInputProvider.
/// This reduces boilerplate when implementing text input for different views.
#[macro_export]
macro_rules! impl_text_input_handler {
    ($ty:ty) => {
        impl gpui_kit::EntityInputHandler for $ty {
            fn text_for_range(
                &mut self,
                range: std::ops::Range<usize>,
                adjusted_range: &mut Option<std::ops::Range<usize>>,
                _window: &mut gpui_kit::Window,
                _cx: &mut gpui_kit::Context<Self>,
            ) -> Option<String> {
                let state = $crate::text_input::TextInputProvider::text_input_state(self)?;
                Some(state.text_for_range(range, adjusted_range))
            }

            fn selected_text_range(
                &mut self,
                _ignore_disabled_input: bool,
                _window: &mut gpui_kit::Window,
                _cx: &mut gpui_kit::Context<Self>,
            ) -> Option<gpui_kit::UTF16Selection> {
                let state = $crate::text_input::TextInputProvider::text_input_state(self)?;
                Some(state.selected_text_range())
            }

            fn marked_text_range(
                &self,
                _window: &mut gpui_kit::Window,
                _cx: &mut gpui_kit::Context<Self>,
            ) -> Option<std::ops::Range<usize>> {
                let state = $crate::text_input::TextInputProvider::text_input_state(self)?;
                state.marked_text_range_utf16()
            }

            fn unmark_text(
                &mut self,
                _window: &mut gpui_kit::Window,
                _cx: &mut gpui_kit::Context<Self>,
            ) {
                if let Some(state) =
                    $crate::text_input::TextInputProvider::text_input_state_mut(self)
                {
                    state.unmark_text();
                }
            }

            fn replace_text_in_range(
                &mut self,
                range: Option<std::ops::Range<usize>>,
                text: &str,
                _window: &mut gpui_kit::Window,
                cx: &mut gpui_kit::Context<Self>,
            ) {
                if let Some(state) =
                    $crate::text_input::TextInputProvider::text_input_state_mut(self)
                {
                    state.replace_text_in_range(range, text);
                    cx.notify();
                }
            }

            fn replace_and_mark_text_in_range(
                &mut self,
                range: Option<std::ops::Range<usize>>,
                new_text: &str,
                new_selected_range: Option<std::ops::Range<usize>>,
                _window: &mut gpui_kit::Window,
                cx: &mut gpui_kit::Context<Self>,
            ) {
                if let Some(state) =
                    $crate::text_input::TextInputProvider::text_input_state_mut(self)
                {
                    state.replace_and_mark_text_in_range(range, new_text, new_selected_range);
                    cx.notify();
                }
            }

            fn bounds_for_range(
                &mut self,
                range_utf16: std::ops::Range<usize>,
                element_bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
                _window: &mut gpui_kit::Window,
                _cx: &mut gpui_kit::Context<Self>,
            ) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
                let state = $crate::text_input::TextInputProvider::text_input_state(self)?;
                Some(state.bounds_for_range(range_utf16, element_bounds))
            }

            fn character_index_for_point(
                &mut self,
                point: gpui_kit::Point<gpui_kit::Pixels>,
                _window: &mut gpui_kit::Window,
                _cx: &mut gpui_kit::Context<Self>,
            ) -> Option<usize> {
                let state = $crate::text_input::TextInputProvider::text_input_state(self)?;
                Some(state.character_index_for_point(point))
            }

            fn accepts_text_input(
                &self,
                _window: &mut gpui_kit::Window,
                _cx: &mut gpui_kit::Context<Self>,
            ) -> bool {
                $crate::text_input::TextInputProvider::text_input_state(self).is_some()
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_range_conversion_handles_multibyte_text() {
        let state = TextInputState::new("a😄é".to_string());
        let utf16 = state.range_to_utf16(&(1..7));
        assert_eq!(utf16, 1..4);
        let utf8 = state.range_from_utf16(&utf16);
        assert_eq!(utf8, 1..7);
    }

    #[test]
    fn replace_text_uses_selection_when_no_range() {
        let mut state = TextInputState::new("hello".to_string());
        state.selected_range = 1..4;
        state.replace_text_in_range(None, "i");
        assert_eq!(state.text(), "hio");
        assert_eq!(state.selected_range(), 2..2);
    }

    #[test]
    fn replace_and_mark_sets_marked_and_selection() {
        let mut state = TextInputState::new("abcd".to_string());
        state.selected_range = 1..3;
        state.replace_and_mark_text_in_range(Some(1..3), "xy", Some(0..1));
        assert_eq!(state.text(), "axyd");
        assert_eq!(state.marked_range, Some(1..3));
        assert_eq!(state.selected_range(), 1..2);
    }

    #[test]
    fn unmark_clears_marked_range() {
        let mut state = TextInputState::new("abc".to_string());
        state.marked_range = Some(0..2);
        state.unmark_text();
        assert_eq!(state.marked_range, None);
    }

    #[test]
    fn delete_to_start_removes_prefix() {
        let mut state = TextInputState::new("hello world".to_string());
        state.set_cursor_utf8(5);
        state.delete_to_start();
        assert_eq!(state.text(), " world");
        assert_eq!(state.selected_range(), 0..0);
    }

    #[test]
    fn delete_word_backward_removes_previous_word() {
        let mut state = TextInputState::new("hello world".to_string());
        state.set_cursor_utf8(11);
        state.delete_word_backward();
        assert_eq!(state.text(), "hello ");
        assert_eq!(state.selected_range(), 6..6);
    }

    #[test]
    fn select_to_utf16_extends_selection() {
        let mut state = TextInputState::new("a😄b".to_string());
        state.set_cursor_utf16(1);
        state.select_to_utf16(4);
        assert_eq!(state.selected_range(), 1..6);
    }

    #[test]
    fn vertical_movement_preserves_unicode_character_column() {
        let mut state = TextInputState::new("ab\n😀x\nqwerty".to_string());
        state.set_cursor_utf8("ab\n😀x\nqwe".len());
        state.move_up();
        assert_eq!(state.cursor_offset(), "ab\n😀x".len());
        state.move_up();
        assert_eq!(state.cursor_offset(), 2);
        state.move_down();
        assert_eq!(state.cursor_offset(), "ab\n😀x".len());
    }

    #[test]
    fn select_token_at_utf16_selects_word_and_whitespace_runs() {
        let mut state = TextInputState::new("hello  world".to_string());

        state.select_token_at_utf16(1);
        assert_eq!(state.selected_range(), 0..5);

        state.select_token_at_utf16(5);
        assert_eq!(state.selected_range(), 5..7);
    }

    #[test]
    fn select_token_at_utf16_handles_punctuation_and_end_of_line() {
        let mut state = TextInputState::new("foo==bar".to_string());

        state.select_token_at_utf16(3);
        assert_eq!(state.selected_range(), 3..5);

        state.select_token_at_utf16(8);
        assert_eq!(state.selected_range(), 5..8);
    }
}
