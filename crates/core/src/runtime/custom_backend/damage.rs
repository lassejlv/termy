//! Render damage: what changed since the last read, and the render read
//! that pairs it with the viewport cells.

use super::*;

impl State {
    pub(super) fn take_damage(&mut self, force_full: bool) -> TerminalRenderDamageSnapshot {
        let (source, source_scrolls) = self.engine.take_render_damage();
        let pending_full = std::mem::take(&mut self.force_full_damage);
        let full = force_full || pending_full;
        let mut damage = if full {
            TerminalDamageSnapshot::Full
        } else {
            match source {
                engine::Damage::Full => TerminalDamageSnapshot::Full,
                engine::Damage::Partial(spans) => TerminalDamageSnapshot::Partial(
                    spans
                        .into_iter()
                        .filter(|span| span.start < span.end)
                        .map(|span| TerminalDirtySpan {
                            row: span.row,
                            left_col: span.start.saturating_sub(1),
                            right_col: span.end.min(self.engine.size().cols.saturating_sub(1)),
                        })
                        .collect(),
                ),
            }
        };
        let scrolls: Vec<_> = if matches!(damage, TerminalDamageSnapshot::Full) {
            Vec::new()
        } else {
            source_scrolls
                .into_iter()
                .map(|scroll| TerminalViewportScroll {
                    top: scroll.top,
                    bottom: scroll.bottom - 1,
                    count: scroll.lines.unsigned_abs() as usize,
                    direction: if scroll.lines > 0 {
                        TerminalViewportScrollDirection::Up
                    } else {
                        TerminalViewportScrollDirection::Down
                    },
                })
                .collect()
        };
        let previous_cursor = self.last_damage_cursor.and_then(|mut cursor| {
            for scroll in &scrolls {
                if !(scroll.top..=scroll.bottom).contains(&cursor.row) {
                    continue;
                }
                cursor.row = match scroll.direction {
                    TerminalViewportScrollDirection::Up => cursor.row.checked_sub(scroll.count)?,
                    TerminalViewportScrollDirection::Down => {
                        cursor.row.checked_add(scroll.count)?
                    }
                };
                if !(scroll.top..=scroll.bottom).contains(&cursor.row) {
                    return None;
                }
            }
            Some(cursor)
        });
        let cursor = self.cursor();
        if cursor != self.last_damage_cursor || !scrolls.is_empty() {
            if let TerminalDamageSnapshot::Partial(spans) = &mut damage {
                let size = self.engine.size();
                let cursors = previous_cursor.into_iter().chain(cursor);
                add_spans(
                    spans,
                    cursors
                        .filter(|cursor| cursor.row < size.rows)
                        .map(|cursor| TerminalDirtySpan {
                            row: cursor.row,
                            left_col: cursor.col.saturating_sub(1),
                            right_col: cursor.col.saturating_add(1).min(size.cols - 1),
                        }),
                );
            }
            self.last_damage_cursor = cursor;
        }
        TerminalRenderDamageSnapshot {
            damage,
            scrolls,
            generation: self.generation,
            palette_revision: self.metadata().palette_revision,
        }
    }

    pub(super) fn render_read(&mut self, force_full: bool) -> TerminalRenderRead {
        let update = self.take_damage(force_full);
        let mut cells =
            Vec::with_capacity(usize::from(self.size.cols) * usize::from(self.size.rows));
        let mut scratch = Vec::new();
        for row in 0..usize::from(self.size.rows) {
            let wrapped = self.engine.viewport_row_wrapped(row);
            self.engine.with_viewport_row(row, &mut scratch, |source| {
                for col in 0..source.len() {
                    cells.push(render_cell(
                        &source,
                        col,
                        wrapped && col + 1 == source.len(),
                    ));
                }
            });
        }
        TerminalRenderRead {
            metadata: self.metadata(),
            palette: self.palette(),
            cells,
            update,
        }
    }
}

// Legacy frame consumers replay only cell patches, so include the rows moved
// by scroll operations rather than silently dropping that part of the update.
pub(super) fn expand_legacy_scroll_damage(update: &mut TerminalRenderDamageSnapshot, cols: usize) {
    if let TerminalDamageSnapshot::Partial(spans) = &mut update.damage {
        let rows = update
            .scrolls
            .iter()
            .flat_map(|scroll| scroll.top..=scroll.bottom);
        add_spans(
            spans,
            rows.map(|row| TerminalDirtySpan {
                row,
                left_col: 0,
                right_col: cols.saturating_sub(1),
            }),
        );
    }
    update.scrolls.clear();
}

/// Add `extra` to `spans` and normalize them. A span that one already in
/// `spans` covers would merge into it unchanged, so it is left out: damage
/// that already covers the additions (a scroll over rows that are dirty
/// across the width) needs no sort, and normalized damage none at all.
pub(super) fn add_spans(
    spans: &mut Vec<TerminalDirtySpan>,
    extra: impl IntoIterator<Item = TerminalDirtySpan>,
) {
    let len = spans.len();
    let sorted = spans.is_sorted_by_key(|span| (span.row, span.left_col));
    // Sorted spans that `normalize_spans` would not merge, as `take_damage`
    // builds them (one per row): normalizing them changes nothing.
    let normalized = sorted
        && spans.windows(2).all(|pair| {
            pair[0].row != pair[1].row || pair[1].left_col > pair[0].right_col.saturating_add(1)
        });
    // Additions mostly come in row order: each walk resumes at the last
    // row found.
    let (mut from, mut last_row) = (0, 0);
    for span in extra {
        if sorted {
            if span.row < last_row {
                from = 0;
            }
            last_row = span.row;
            let existing = &spans[..len];
            while existing.get(from).is_some_and(|other| other.row < span.row) {
                from += 1;
            }
            let covered = existing[from..]
                .iter()
                .take_while(|other| other.row == span.row)
                .any(|other| {
                    other.left_col <= span.left_col
                        && span.left_col <= other.right_col
                        && span.right_col <= other.right_col
                });
            if covered {
                continue;
            }
        }
        spans.push(span);
    }
    if !normalized || spans.len() != len {
        normalize_spans(spans);
    }
}

pub(super) fn normalize_spans(spans: &mut Vec<TerminalDirtySpan>) {
    // Spans arrive as a few sorted runs (rows, then scrolled rows or cursor
    // cells), which the stable sort merges in linear time. Equal keys merge
    // to the same span in either order.
    let key = |span: &TerminalDirtySpan| (span.row, span.left_col);
    if !spans.is_sorted_by_key(key) {
        spans.sort_by_key(key);
    }
    let mut len = 0;
    for index in 0..spans.len() {
        let span = spans[index];
        if len > 0
            && spans[len - 1].row == span.row
            && span.left_col <= spans[len - 1].right_col.saturating_add(1)
        {
            spans[len - 1].right_col = spans[len - 1].right_col.max(span.right_col);
        } else {
            spans[len] = span;
            len += 1;
        }
    }
    spans.truncate(len);
}
