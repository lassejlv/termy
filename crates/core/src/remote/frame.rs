//! Per-subscription frame deltas (audit I1). The host sends each subscriber
//! the rows that changed since the frame it sent that subscriber before, and
//! a full viewport only when the subscriber has no frame of the same screen,
//! size and palette. Rows are compared against the previous frame shifted by
//! the lines output scrolled in between, so scrolling output costs the new
//! rows, not the whole screen.
//!
//! The client applies a delta to the frame it shows and reports what moved
//! and what changed as render damage, so the renderer replays the scroll and
//! repaints the changed cells instead of diffing the viewport.
use super::*;
use crate::{TerminalDirtySpan, TerminalViewportScroll, TerminalViewportScrollDirection};
use anyhow::{Context, ensure};

/// One frame of a subscription. See the module documentation.
#[derive(Serialize, Deserialize)]
pub struct FrameUpdate {
    /// The new state. For a delta, `render.cells` holds only the cells of
    /// `delta.rows`, one row after another.
    state: RemoteState,
    delta: Option<RowDelta>,
}

#[derive(Serialize, Deserialize)]
struct RowDelta {
    /// The generation of the frame the delta applies to.
    base: u64,
    /// The rows the content moved up since that frame (down when negative).
    shift: i32,
    /// The rows whose cells the update carries, in ascending order.
    rows: Vec<u16>,
}

/// What a frame changed in the viewport of the frame before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FrameChange {
    /// Everything may have changed.
    Full,
    /// The content moved up by `shift` rows (down when negative), then the
    /// cells in `spans` changed. Spans use the new frame's rows.
    Rows {
        shift: i32,
        spans: Vec<TerminalDirtySpan>,
    },
}

/// The host side of one subscription: the frame sent last is the base of the
/// next delta.
#[derive(Default)]
pub struct FrameEncoder {
    /// The frame sent last, without its images: a paused view must not keep
    /// pixels the pane no longer shows.
    last: Option<RemoteState>,
}

impl FrameEncoder {
    pub fn encode(&mut self, state: &RemoteState) -> FrameUpdate {
        let update = match self.last.as_ref() {
            Some(base) if same_screen(base, state) => delta(base, state),
            _ => FrameUpdate {
                state: with_cells(state, state.render.cells.clone()),
                delta: None,
            },
        };
        self.last = Some(with_cells(state, state.render.cells.clone()));
        update
    }

    /// Forgets the frame sent last, so the next frame is sent in full: the
    /// client lost track of it and asked for a full frame.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

impl FrameUpdate {
    /// Whether the update carries the whole viewport.
    pub(crate) fn is_full(&self) -> bool {
        self.delta.is_none()
    }

    /// The state this update describes, built on `base`, the frame the
    /// client shows, and what changed since it.
    pub(crate) fn apply(self, base: &RemoteState) -> anyhow::Result<(RemoteState, FrameChange)> {
        let Some(delta) = self.delta else {
            return Ok((self.state, FrameChange::Full));
        };
        let mut state = self.state;
        let metadata = state.render.metadata;
        let (cols, rows) = (usize::from(metadata.cols), usize::from(metadata.rows));
        ensure!(
            base.render.metadata.generation == delta.base
                && base.render.metadata.cols == metadata.cols
                && base.render.metadata.rows == metadata.rows
                && base.render.cells.len() == cols * rows,
            "terminal frame does not follow the one shown"
        );
        ensure!(
            delta.rows.windows(2).all(|pair| pair[0] < pair[1])
                && delta.rows.last().is_none_or(|row| usize::from(*row) < rows)
                && state.render.cells.len() == delta.rows.len() * cols,
            "invalid terminal frame rows"
        );
        let changed = std::mem::take(&mut state.render.cells);
        let mut cells = Vec::with_capacity(cols * rows);
        let mut spans = Vec::with_capacity(delta.rows.len());
        let mut updates = delta.rows.iter().zip(changed.chunks(cols)).peekable();
        for row in 0..rows {
            let source = source_row(row, delta.shift, rows)
                .map(|source| &base.render.cells[source * cols..][..cols]);
            let Some((_, new)) = updates.next_if(|(updated, _)| usize::from(**updated) == row)
            else {
                cells.extend_from_slice(source.context("terminal frame lacks a new row")?);
                continue;
            };
            // Repaint only the columns that differ from the row that moved
            // here; a row with no source is new.
            let (left, right) = match source {
                Some(old) => (
                    new.iter().zip(old).position(|(a, b)| a != b),
                    new.iter().zip(old).rposition(|(a, b)| a != b),
                ),
                None => (Some(0), Some(cols - 1)),
            };
            if let (Some(left_col), Some(right_col)) = (left, right) {
                spans.push(TerminalDirtySpan {
                    row,
                    left_col,
                    right_col,
                });
            }
            cells.extend_from_slice(new);
        }
        state.render.cells = cells;
        Ok((
            state,
            FrameChange::Rows {
                shift: delta.shift,
                spans,
            },
        ))
    }
}

/// Whether a delta from `base` can describe `state`: the same grid, screen
/// and palette.
fn same_screen(base: &RemoteState, state: &RemoteState) -> bool {
    let (old, new) = (base.render.metadata, state.render.metadata);
    old.cols == new.cols
        && old.rows == new.rows
        && base.alternate_screen == state.alternate_screen
        && base.render.palette == state.render.palette
        && base.render.cells.len() == state.render.cells.len()
}

/// The row of the previous frame that shows what row `row` shows after the
/// content moved up by `shift` rows.
fn source_row(row: usize, shift: i32, rows: usize) -> Option<usize> {
    let source = row as i64 + i64::from(shift);
    (0..rows as i64)
        .contains(&source)
        .then_some(source as usize)
}

/// How far the content moved up from `base` to `state`: the lines output
/// scrolled into history, less the change in how far the view is scrolled
/// back. Zero when that is a whole screen or more.
fn content_shift(base: &RemoteState, state: &RemoteState) -> i32 {
    let top = |state: &RemoteState| {
        i128::from(state.lines_scrolled) - state.render.metadata.display_offset as i128
    };
    let shift = top(state) - top(base);
    if shift.unsigned_abs() >= u128::from(state.render.metadata.rows) {
        0
    } else {
        shift as i32
    }
}

fn delta(base: &RemoteState, state: &RemoteState) -> FrameUpdate {
    let cols = usize::from(state.render.metadata.cols);
    let rows = usize::from(state.render.metadata.rows);
    let changed_rows = |shift: i32| -> Vec<u16> {
        (0..rows)
            .filter(|&row| {
                let new = &state.render.cells[row * cols..][..cols];
                source_row(row, shift, rows)
                    .is_none_or(|source| base.render.cells[source * cols..][..cols] != *new)
            })
            .map(|row| row as u16)
            .collect()
    };
    // The scroll count says how output moved the content. Scrolling inside
    // a region (a pager or an editor) does not count, so not moving at all
    // may match better.
    let shift = content_shift(base, state);
    let mut best = (0, changed_rows(0));
    if shift != 0 {
        let shifted = changed_rows(shift);
        if shifted.len() < best.1.len() {
            best = (shift, shifted);
        }
    }
    let (shift, rows) = best;
    let mut cells = Vec::with_capacity(rows.len() * cols);
    for row in &rows {
        cells.extend_from_slice(&state.render.cells[usize::from(*row) * cols..][..cols]);
    }
    FrameUpdate {
        state: with_cells(state, cells),
        delta: Some(RowDelta {
            base: base.render.metadata.generation,
            shift,
            rows,
        }),
    }
}

/// A copy of `state` with `cells` in place of its viewport.
fn with_cells(state: &RemoteState, cells: Vec<TerminalRenderCell>) -> RemoteState {
    RemoteState {
        render: TerminalRenderRead {
            metadata: state.render.metadata,
            palette: state.render.palette.clone(),
            cells,
            update: state.render.update.clone(),
        },
        size: state.size,
        child_pid: state.child_pid,
        cursor_position: state.cursor_position,
        mouse_mode: state.mouse_mode,
        keyboard_mode: state.keyboard_mode,
        bracketed_paste: state.bracketed_paste,
        alternate_screen: state.alternate_screen,
        clipboard_paste_events: state.clipboard_paste_events,
        graphics_revision: state.graphics_revision,
        graphics_deadline: state.graphics_deadline,
        lines_scrolled: state.lines_scrolled,
        graphics: None,
    }
}

/// Render damage collected from the frames a client received since its
/// renderer last read one. See [`RemoteTransport::state_with_damage`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteDamage {
    /// The generation of the frame the renderer read last; the damage
    /// applies only to that frame.
    pub base: u64,
    pub scrolls: Vec<TerminalViewportScroll>,
    pub spans: Vec<TerminalDirtySpan>,
}

/// More scrolls than this between two renders are not worth replaying.
const MAX_PENDING_SCROLLS: usize = 32;

/// Collects [`FrameChange`]s between two renders.
#[derive(Debug, Default)]
pub(crate) struct DamageTracker {
    /// `None` when the damage is unknown, for example after a full frame.
    pending: Option<PendingDamage>,
}

#[derive(Debug)]
struct PendingDamage {
    base: u64,
    scrolls: Vec<TerminalViewportScroll>,
    /// The changed columns of each row, in the current frame's rows.
    rows: Vec<Option<(usize, usize)>>,
}

impl DamageTracker {
    /// Records the change from the frame shown before to `state`.
    pub(crate) fn record(&mut self, change: FrameChange, state: &RemoteState) {
        let rows = usize::from(state.render.metadata.rows);
        let Some(pending) = self
            .pending
            .as_mut()
            .filter(|pending| pending.rows.len() == rows)
        else {
            self.pending = None;
            return;
        };
        let FrameChange::Rows { shift, spans } = change else {
            self.pending = None;
            return;
        };
        if shift != 0 {
            if pending.scrolls.len() >= MAX_PENDING_SCROLLS {
                self.pending = None;
                return;
            }
            let count = shift.unsigned_abs() as usize;
            pending.scrolls.push(TerminalViewportScroll {
                top: 0,
                bottom: rows - 1,
                count,
                direction: if shift > 0 {
                    TerminalViewportScrollDirection::Up
                } else {
                    TerminalViewportScrollDirection::Down
                },
            });
            // Damage moves with the rows; rows that scroll in are in `spans`.
            if shift > 0 {
                pending.rows.rotate_left(count);
                pending.rows[rows - count..].fill(None);
            } else {
                pending.rows.rotate_right(count);
                pending.rows[..count].fill(None);
            }
        }
        for span in spans {
            let row = &mut pending.rows[span.row];
            *row = Some(match *row {
                Some((left, right)) => (left.min(span.left_col), right.max(span.right_col)),
                None => (span.left_col, span.right_col),
            });
        }
    }

    /// The damage since the last call, which starts collecting anew from
    /// `state`, the frame the renderer reads now.
    pub(crate) fn take(&mut self, state: &RemoteState) -> Option<RemoteDamage> {
        let next = PendingDamage {
            base: state.render.metadata.generation,
            scrolls: Vec::new(),
            rows: vec![None; usize::from(state.render.metadata.rows)],
        };
        let pending = self.pending.replace(next)?;
        Some(RemoteDamage {
            base: pending.base,
            scrolls: pending.scrolls,
            spans: pending
                .rows
                .into_iter()
                .enumerate()
                .filter_map(|(row, columns)| {
                    columns.map(|(left_col, right_col)| TerminalDirtySpan {
                        row,
                        left_col,
                        right_col,
                    })
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal(cols: u16, rows: u16) -> Terminal {
        Terminal::new_display(
            TerminalSize {
                cols,
                rows,
                ..TerminalSize::default()
            },
            None,
        )
    }

    fn capture(terminal: &Terminal) -> Arc<RemoteState> {
        Arc::new(RemoteState::capture(terminal))
    }

    fn encoded_len(update: &FrameUpdate) -> usize {
        bincode::serialize(update).unwrap().len()
    }

    /// Sends `state` through `encoder` and the wire, and applies it on
    /// `shown` like a client.
    fn roundtrip(
        encoder: &mut FrameEncoder,
        shown: &RemoteState,
        state: &Arc<RemoteState>,
    ) -> (RemoteState, FrameChange, usize) {
        let update = encoder.encode(state);
        let bytes = encoded_len(&update);
        let update: FrameUpdate =
            bincode::deserialize(&bincode::serialize(&update).unwrap()).unwrap();
        let (applied, change) = update.apply(shown).unwrap();
        assert!(
            applied.same_frame(state),
            "the client frame differs from the host's"
        );
        (applied, change, bytes)
    }

    /// A busy, colorful 200x50 screen.
    fn busy_screen() -> Terminal {
        let terminal = terminal(200, 50);
        for row in 0..50 {
            let mut line = String::new();
            for col in 0..200 {
                line.push_str(&format!(
                    "\x1b[38;2;{};{};{}m{}",
                    row * 5,
                    col,
                    (row + col) % 256,
                    char::from(b'a' + ((row + col) % 26) as u8)
                ));
            }
            terminal.feed_output(line.as_bytes());
        }
        terminal
    }

    #[test]
    fn a_one_line_change_sends_far_fewer_bytes_than_a_full_frame() {
        let terminal = busy_screen();
        let mut encoder = FrameEncoder::default();
        let first = capture(&terminal);
        let (shown, change, full_bytes) = roundtrip(&mut encoder, &first, &first);
        assert_eq!(change, FrameChange::Full);
        terminal.feed_output(b"\x1b[20;10Hchanged");
        let (_, change, delta_bytes) = roundtrip(&mut encoder, &shown, &capture(&terminal));
        eprintln!("full frame {full_bytes} bytes, one-line delta {delta_bytes} bytes");
        assert!(
            delta_bytes * 25 < full_bytes,
            "delta {delta_bytes} bytes, full frame {full_bytes} bytes"
        );
        assert_eq!(
            change,
            FrameChange::Rows {
                shift: 0,
                spans: vec![TerminalDirtySpan {
                    row: 19,
                    left_col: 9,
                    right_col: 15,
                }],
            }
        );
    }

    #[test]
    fn scrolling_output_sends_only_the_new_rows() {
        // A small history limit: the count, not the history size, tells
        // how far the content moved once history is full.
        let terminal = terminal(20, 6);
        terminal.set_scrollback_history(3);
        for line in 0..12 {
            terminal.feed_output(format!("\r\nline {line}").as_bytes());
        }
        let mut encoder = FrameEncoder::default();
        let first = capture(&terminal);
        let (shown, _, _) = roundtrip(&mut encoder, &first, &first);
        terminal.feed_output(b"\r\nnext\r\nlast");
        let state = capture(&terminal);
        assert_eq!(
            state.render.metadata.history_size,
            shown.render.metadata.history_size
        );
        let (_, change, _) = roundtrip(&mut encoder, &shown, &state);
        let FrameChange::Rows { shift, spans } = change else {
            panic!("expected a delta");
        };
        assert_eq!(shift, 2);
        assert_eq!(
            spans.iter().map(|span| span.row).collect::<Vec<_>>(),
            [4, 5]
        );
    }

    #[test]
    fn scrolling_back_through_history_moves_rows_down() {
        let mut terminal = terminal(20, 6);
        for line in 0..30 {
            terminal.feed_output(format!("\r\nline {line}").as_bytes());
        }
        let mut encoder = FrameEncoder::default();
        let first = capture(&terminal);
        let (shown, _, _) = roundtrip(&mut encoder, &first, &first);
        execute(&mut terminal, RemoteCommand::Scroll(2));
        let (_, change, _) = roundtrip(&mut encoder, &shown, &capture(&terminal));
        let FrameChange::Rows { shift, spans } = change else {
            panic!("expected a delta");
        };
        assert_eq!(shift, -2);
        assert_eq!(
            spans.iter().map(|span| span.row).collect::<Vec<_>>(),
            [0, 1]
        );
    }

    #[test]
    fn screen_switches_resizes_and_palette_changes_send_full_frames() {
        let mut terminal = terminal(20, 6);
        terminal.feed_output(b"text");
        let mut encoder = FrameEncoder::default();
        let first = capture(&terminal);
        let (mut shown, _, _) = roundtrip(&mut encoder, &first, &first);
        let changes: [&dyn Fn(&mut Terminal); 3] = [
            &|terminal| terminal.feed_output(b"\x1b[?1049h"),
            &|terminal| {
                terminal.resize(TerminalSize {
                    cols: 21,
                    rows: 6,
                    ..TerminalSize::default()
                });
            },
            &|terminal| terminal.feed_output(b"\x1b]4;1;rgb:12/34/56\x1b\\"),
        ];
        for change in changes {
            change(&mut terminal);
            let (applied, kind, _) = roundtrip(&mut encoder, &shown, &capture(&terminal));
            assert_eq!(kind, FrameChange::Full);
            shown = applied;
        }
    }

    #[test]
    fn a_delta_for_another_frame_is_rejected() {
        let terminal = terminal(20, 6);
        let mut encoder = FrameEncoder::default();
        let first = capture(&terminal);
        encoder.encode(&first);
        terminal.feed_output(b"a");
        let stale = capture(&terminal);
        encoder.encode(&stale);
        terminal.feed_output(b"b");
        let update = encoder.encode(&capture(&terminal));
        assert!(update.apply(&first).is_err());
    }

    #[test]
    fn damage_follows_rows_through_scrolls_until_taken() {
        let terminal = terminal(20, 6);
        let state = capture(&terminal);
        let mut tracker = DamageTracker::default();
        assert_eq!(tracker.take(&state), None, "nothing was rendered yet");
        let span = |row, left_col, right_col| TerminalDirtySpan {
            row,
            left_col,
            right_col,
        };
        tracker.record(
            FrameChange::Rows {
                shift: 0,
                spans: vec![span(3, 2, 4)],
            },
            &state,
        );
        tracker.record(
            FrameChange::Rows {
                shift: 1,
                spans: vec![span(2, 0, 1), span(5, 0, 19)],
            },
            &state,
        );
        let damage = tracker.take(&state).unwrap();
        assert_eq!(damage.base, state.render.metadata.generation);
        assert_eq!(damage.scrolls.len(), 1);
        assert_eq!(damage.spans, [span(2, 0, 4), span(5, 0, 19)]);
        assert_eq!(tracker.take(&state).unwrap().spans, []);
        tracker.record(FrameChange::Full, &state);
        assert_eq!(tracker.take(&state), None);
    }
}
