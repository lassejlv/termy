use super::TextInputState;
use gpui_kit::{Bounds, Pixels, ShapedLine, point, px};
use std::ops::{Deref, DerefMut, Range};

/// Inline overlays share editing behavior with settings and plugin fields,
/// while retaining their candidate-window and hit-testing geometry.
#[derive(Clone, Debug)]
pub(crate) struct InlineInputState {
    state: TextInputState,
}

impl Deref for InlineInputState {
    type Target = TextInputState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for InlineInputState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl InlineInputState {
    pub(crate) fn new(text: String) -> Self {
        Self {
            state: TextInputState::new(text),
        }
    }

    pub(crate) fn line_offsets(&self) -> impl Iterator<Item = Pixels> + '_ {
        self.state
            .last_line_metas
            .iter()
            .map(|(_, _, offset)| *offset)
    }

    pub(crate) fn update_layout_cache(
        &mut self,
        bounds: Bounds<Pixels>,
        layout: Option<ShapedLine>,
        line_offset_x: Pixels,
        line_metas: Vec<(usize, Bounds<Pixels>, Pixels)>,
        line_layouts: Vec<Option<ShapedLine>>,
    ) {
        self.state.last_bounds = Some(bounds);
        self.state.last_layout = layout;
        self.state.last_line_offset_x = line_offset_x;
        self.state.last_line_metas = line_metas;
        self.state.last_line_layouts = line_layouts;
    }

    pub(crate) fn bounds_for_range(
        &self,
        range_utf16: Range<usize>,
        fallback_bounds: Bounds<Pixels>,
    ) -> Bounds<Pixels> {
        let bounds = self.state.last_bounds.unwrap_or(fallback_bounds);
        let range = self.range_from_utf16(&range_utf16);
        let (start_x, end_x) = if let Some(layout) = self.state.last_layout.as_ref() {
            (
                layout.x_for_index(range.start),
                layout.x_for_index(range.end),
            )
        } else {
            (px(0.0), px(0.0))
        };

        Bounds::from_corners(
            point(
                bounds.left() + self.state.last_line_offset_x + start_x,
                bounds.top(),
            ),
            point(
                bounds.left() + self.state.last_line_offset_x + end_x,
                bounds.bottom(),
            ),
        )
    }

    pub(crate) fn character_index_for_point(&self, point: gpui_kit::Point<Pixels>) -> usize {
        if self.state.text.is_empty() {
            return 0;
        }

        // Multiline-aware path: find which row the point is on
        if !self.state.last_line_metas.is_empty() {
            // Find the matching row by y coordinate
            let row_idx = self
                .state
                .last_line_metas
                .iter()
                .enumerate()
                .find(|(_, (_, bounds, _))| point.y >= bounds.top() && point.y <= bounds.bottom())
                .map_or_else(
                    || {
                        if point.y
                            < self
                                .state
                                .last_line_metas
                                .first()
                                .map_or(px(0.0), |(_, b, _)| b.top())
                        {
                            0
                        } else {
                            self.state.last_line_metas.len() - 1
                        }
                    },
                    |(i, _)| i,
                );

            let (line_start_byte, row_bounds, offset_x) = &self.state.last_line_metas[row_idx];
            let layout = self
                .state
                .last_line_layouts
                .get(row_idx)
                .and_then(|l| l.as_ref());

            let text_left = row_bounds.left() + *offset_x;
            let local_x = if point.x <= text_left {
                px(0.0)
            } else {
                point.x - text_left
            };

            let utf8_index_in_row = layout.map_or(0, |l| l.closest_index_for_x(local_x));
            let abs_utf8 = line_start_byte + utf8_index_in_row;
            return self.utf8_to_utf16(abs_utf8);
        }

        // Single-line fallback
        let Some(bounds) = self.state.last_bounds else {
            return self.range_to_utf16(&self.state.selected_range).start;
        };

        if point.y < bounds.top() {
            return 0;
        }
        if point.y > bounds.bottom() {
            return self.utf8_to_utf16(self.state.text.len());
        }

        let text_left = bounds.left() + self.state.last_line_offset_x;
        let local_x = if point.x <= text_left {
            px(0.0)
        } else {
            point.x - text_left
        };

        let utf8_index = self
            .state
            .last_layout
            .as_ref()
            .map_or(0, |layout| layout.closest_index_for_x(local_x));
        self.utf8_to_utf16(utf8_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::size;

    fn bounds(y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(10.0), px(y)), size(px(100.0), px(10.0)))
    }

    fn multiline_state() -> InlineInputState {
        let mut state = InlineInputState::new("a😄\nb".to_string());
        state.update_layout_cache(
            bounds(10.0),
            None,
            px(4.0),
            vec![(0, bounds(10.0), px(0.0)), (6, bounds(30.0), px(0.0))],
            vec![None, None],
        );
        state
    }

    #[test]
    fn inline_hit_testing_keeps_the_existing_row_fallbacks() {
        let state = multiline_state();
        for (y, offset) in [(0.0, 0), (15.0, 0), (25.0, 4), (35.0, 4), (50.0, 4)] {
            assert_eq!(
                state.character_index_for_point(point(px(10.0), px(y))),
                offset
            );
        }
    }

    #[test]
    fn inline_candidate_bounds_keep_the_overlay_anchor() {
        let state = multiline_state();
        let candidate = state.bounds_for_range(4..5, bounds(100.0));
        assert_eq!(
            candidate,
            Bounds::from_corners(point(px(14.0), px(10.0)), point(px(14.0), px(20.0)))
        );
    }

    #[test]
    fn shared_edits_invalidate_inline_row_layouts() {
        let mut state = multiline_state();
        assert_eq!(state.line_offsets().count(), 2);
        state.delete_backward();
        assert_eq!(state.text(), "a😄\n");
        assert_eq!(state.line_offsets().count(), 0);
        state.replace_and_mark_text_in_range(None, "x", Some(0..1));
        assert_eq!(state.marked_text_range_utf16(), Some(4..5));
        state.unmark_text();
        assert_eq!(state.marked_text_range_utf16(), None);
    }
}
