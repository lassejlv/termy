//! Reads for the session host: the published remote state, line texts,
//! and links on a row of a frame that a client already holds.

use super::*;

impl CustomBackend {
    /// [`Self::hyperlink_at`] for a row of the frame `view`, found in the
    /// current viewport under the same lock as the lookup (audit L19).
    pub(in crate::runtime) fn hyperlink_at_view(
        &self,
        row: usize,
        col: usize,
        view: crate::remote::ViewportAnchor,
    ) -> Option<DetectedLink> {
        let state = self.shared.state();
        let row = view.row_in(row, &state.viewport_anchor(), |current| {
            state.viewport_row_hash(current)
        })?;
        crate::links::hyperlink_at_viewport_cell(&state.engine, row, col)
    }

    /// [`Self::link_at`] for a row of the frame `view`. The link's rows are
    /// numbered as in that frame.
    pub(in crate::runtime) fn link_at_view(
        &self,
        row: usize,
        col: usize,
        view: crate::remote::ViewportAnchor,
    ) -> Option<DetectedViewportLink> {
        let (current, candidate) = {
            let state = self.shared.state();
            let current = view.row_in(row, &state.viewport_anchor(), |current| {
                state.viewport_row_hash(current)
            })?;
            (
                current,
                crate::links::link_candidate_at_viewport_cell(&state.engine, current, col),
            )
        };
        let mut link = candidate?.resolve()?;
        let last = usize::from(view.rows).saturating_sub(1);
        let to_frame = |current_row: usize| (current_row + row).saturating_sub(current).min(last);
        link.start_row = to_frame(link.start_row);
        link.end_row = to_frame(link.end_row);
        Some(link)
    }

    /// Lines `first..=last` as text, at most `max_bytes` of it. With an
    /// `anchor` (an earlier reply's `lines_scrolled`), lines are numbered as
    /// they were then.
    pub(in crate::runtime) fn line_texts(
        &self,
        first: i32,
        last: i32,
        anchor: Option<u64>,
        max_bytes: usize,
    ) -> crate::remote::LineTexts {
        let state = self.shared.state();
        let lines_scrolled = state.engine.scrolled_into_history();
        // Output since the anchor moved every line up by this many rows.
        let moved = anchor.map_or(0, |anchor| lines_scrolled.wrapping_sub(anchor) as i64);
        let to_now =
            |line: i32| (i64::from(line) - moved).clamp(i32::MIN.into(), i32::MAX.into()) as i32;
        let to_then =
            |line: i32| (i64::from(line) + moved).clamp(i32::MIN.into(), i32::MAX.into()) as i32;
        let (top, bottom) = (
            -(state.engine.history_size() as i32),
            i32::from(state.size.rows) - 1,
        );
        let start = to_now(first).max(top);
        let end = to_now(last).min(bottom);
        // Newest first: history drops its oldest lines while a client reads
        // the rest, so what remains to read stays contiguous.
        let mut lines = Vec::new();
        let mut bytes = 0;
        let mut scratch = Vec::new();
        let mut next = end;
        while next >= start && bytes < max_bytes {
            let wrapped = state.engine.line_wrapped(next);
            let Some(text) = state
                .engine
                .with_line(next, &mut scratch, |cells| line_text(&cells, wrapped))
            else {
                break;
            };
            bytes += text.text.len() + text.columns.len();
            lines.push(text);
            next -= 1;
        }
        lines.reverse();
        crate::remote::LineTexts {
            bounds: (to_then(top), to_then(bottom), usize::from(state.size.cols)),
            first: to_then(next + 1),
            lines_scrolled: anchor.unwrap_or(lines_scrolled),
            lines,
            scrolled: lines_scrolled,
            rows: state.size.rows,
        }
    }

    /// Everything a session host publishes, read under one engine lock so
    /// that images, cells and screen always describe the same moment (audit
    /// L20).
    pub(in crate::runtime) fn remote_state(&self) -> crate::remote::RemoteState {
        let child_pid = self.child_pid();
        let mut state = self.shared.state();
        let (graphics_revision, graphics) = state.engine.graphics_snapshot();
        let alternate_screen = state.engine.alternate_screen();
        let render = state.render_read(true);
        let cursor = state.engine.cursor();
        let modes = state.engine.modes();
        crate::remote::RemoteState {
            render,
            size: state.size,
            child_pid,
            cursor_position: (cursor.col, cursor.row),
            mouse_mode: mouse_mode(modes),
            keyboard_mode: keyboard_mode(modes),
            bracketed_paste: modes.bracketed_paste,
            alternate_screen,
            clipboard_paste_events: modes.clipboard_paste_events,
            graphics_revision,
            graphics_deadline: graphics
                .iter()
                .filter_map(|placement| placement.animation_deadline)
                .min(),
            lines_scrolled: state.engine.scrolled_into_history(),
            graphics: Some(graphics),
        }
    }
}

/// A line as [`crate::remote::LineText`]: each cell's text, a space for a
/// blank, hidden or control cell, nothing for a wide character's spacer.
fn line_text(cells: &engine::Line<'_>, wrapped: bool) -> crate::remote::LineText {
    let mut text = String::with_capacity(cells.len());
    let mut columns = Vec::with_capacity(cells.len());
    // The text and columns up to the last cell that is not blank.
    let mut kept = (0, 0);
    for col in 0..cells.len() {
        let cell = &cells[col];
        let start = text.len();
        if cell.flags & (engine::Cell::WIDE_SPACER | engine::Cell::LEADING_WIDE_SPACER) != 0 {
            columns.push(0);
            kept = (text.len(), columns.len());
            continue;
        }
        let character = cell.character;
        if cell.style.attributes & engine::Style::HIDDEN != 0
            || character == '\0'
            || character.is_control()
        {
            text.push(' ');
        } else {
            text.push(character);
            for combining in cells.combining(col).chars() {
                if text.len() - start + combining.len_utf8() > usize::from(u8::MAX) {
                    break;
                }
                text.push(combining);
            }
        }
        columns.push((text.len() - start) as u8);
        if &text[start..] != " " {
            kept = (text.len(), columns.len());
        }
    }
    text.truncate(kept.0);
    columns.truncate(kept.1);
    crate::remote::LineText {
        text,
        columns,
        wrapped,
    }
}

impl State {
    fn viewport_anchor(&self) -> crate::remote::ViewportAnchor {
        crate::remote::ViewportAnchor {
            generation: self.generation,
            display_offset: self.engine.display_offset(),
            history_size: self.engine.history_size(),
            lines_scrolled: self.engine.scrolled_into_history(),
            alternate_screen: self.engine.alternate_screen(),
            cols: self.size.cols,
            rows: self.size.rows,
            row_text: 0,
        }
    }

    /// [`crate::remote::row_text_hash`] of viewport row `row`, from the same
    /// cell texts a captured frame carries.
    fn viewport_row_hash(&self, row: usize) -> u64 {
        let mut scratch = Vec::new();
        self.engine
            .with_viewport_row(row, &mut scratch, |line| {
                let texts: Vec<_> = (0..line.len())
                    .map(|col| {
                        TerminalRenderText::from_cell_suffix(
                            line[col].character,
                            Some(line.combining(col)),
                        )
                    })
                    .collect();
                crate::remote::row_text_hash(texts.iter().map(TerminalRenderText::as_str))
            })
            .unwrap_or_default()
    }
}
