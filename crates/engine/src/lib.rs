mod cell;
mod color;
mod damage;
mod emulator;
mod event;
mod frame;
mod grid;
mod input;
mod pointer;
pub mod pty;
mod search;
mod selection;

pub use cell::{Cell, CellFlags};
pub use color::{Color, DEFAULT_BACKGROUND_RGB, DEFAULT_CURSOR_RGB, DEFAULT_FOREGROUND_RGB};
pub use event::{DynamicColor, MousePointerShape, TerminalEvent};
pub use frame::{CursorShape, CursorState, FrameUpdate, RowMove, RowMoveDirection, RowUpdate};
pub use input::{
    Key, KeyEvent, KeyEventKind, KeypadKey, KittyKeyboardFlags, MediaKey, ModifierKey, Modifiers,
    MouseButton, MouseEvent, MouseEventKind, MouseTrackingMode, encode_mouse,
};
pub use search::{SearchDirection, SearchMatch, SearchOptions};
pub use selection::{SelectionMode, SelectionPoint, SelectionRange};
pub use smol_str::SmolStr;

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use emulator::Emulator;
use serde::{Deserialize, Serialize};

const TERMINAL_SNAPSHOT_VERSION: u16 = 2;

#[derive(Serialize)]
struct TerminalSnapshotRef<'a> {
    version: u16,
    emulator: &'a Emulator,
}

#[derive(Deserialize)]
struct TerminalSnapshot {
    version: u16,
    emulator: Emulator,
}

#[derive(Clone, Copy, Debug)]
pub struct TerminalConfig {
    pub columns: usize,
    pub rows: usize,
    pub scrollback_limit: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            columns: 80,
            rows: 24,
            scrollback_limit: 10_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalMetrics {
    pub feed_calls: u64,
    pub bytes_fed: u64,
    pub frame_requests: u64,
    pub damaged_frames: u64,
    pub full_frames: u64,
    pub row_moves: u64,
    pub rows_moved: u64,
    pub row_updates: u64,
    pub cells_copied: u64,
}

/// Allocation-capacity counters for the terminal's main and alternate grids.
///
/// These counters are computed only when requested and do not add work to the feed or frame paths.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalMemoryStats {
    pub live_rows: usize,
    pub scrollback_rows: usize,
    pub spare_rows: usize,
    pub live_cell_capacity: usize,
    pub scrollback_cell_capacity: usize,
    pub spare_cell_capacity: usize,
    pub live_row_capacity: usize,
    pub scrollback_row_capacity: usize,
    pub damage_row_capacity: usize,
    pub damage_snapshot_capacity: usize,
}

impl TerminalMemoryStats {
    #[must_use]
    pub const fn total_cell_capacity(self) -> usize {
        self.live_cell_capacity
            .saturating_add(self.scrollback_cell_capacity)
            .saturating_add(self.spare_cell_capacity)
    }

    #[must_use]
    pub const fn cell_capacity_bytes(self) -> usize {
        self.total_cell_capacity()
            .saturating_mul(std::mem::size_of::<Cell>())
    }
}

pub struct Terminal {
    parser: vte::Parser,
    emulator: Emulator,
    metrics: TerminalMetrics,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Terminal")
            .field("emulator", &self.emulator)
            .finish_non_exhaustive()
    }
}

impl Terminal {
    #[must_use]
    pub fn new(config: TerminalConfig) -> Self {
        Self {
            parser: vte::Parser::new(),
            emulator: Emulator::new(config.columns, config.rows, config.scrollback_limit),
            metrics: TerminalMetrics::default(),
        }
    }

    /// Serializes the terminal emulator so another process can resume rendering the same buffer.
    ///
    /// The VT parser itself is deliberately recreated at a ground state. Callers should snapshot
    /// only after draining currently available PTY output, as the multiplexer does.
    ///
    /// # Errors
    ///
    /// Returns an error if the emulator cannot be encoded.
    pub fn snapshot(&self) -> Result<Vec<u8>> {
        bincode::serde::encode_to_vec(
            TerminalSnapshotRef {
                version: TERMINAL_SNAPSHOT_VERSION,
                emulator: &self.emulator,
            },
            bincode::config::standard(),
        )
        .context("serializing terminal snapshot")
    }

    /// Restores a terminal emulator produced by [`Terminal::snapshot`].
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, trailing, or unsupported snapshot data.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self> {
        let (snapshot, consumed): (TerminalSnapshot, usize) =
            bincode::serde::decode_from_slice(bytes, bincode::config::standard())
                .context("deserializing terminal snapshot")?;
        if consumed != bytes.len() {
            bail!("terminal snapshot contains trailing data");
        }
        if snapshot.version != TERMINAL_SNAPSHOT_VERSION {
            bail!("unsupported terminal snapshot version {}", snapshot.version);
        }
        Ok(Self {
            parser: vte::Parser::new(),
            emulator: snapshot.emulator,
            metrics: TerminalMetrics::default(),
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.metrics.feed_calls = self.metrics.feed_calls.saturating_add(1);
        self.metrics.bytes_fed = self
            .metrics
            .bytes_fed
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let viewport_revision = self.emulator.viewport_revision();
        self.parser.advance(&mut self.emulator, bytes);
        self.emulator.invalidate_search();
        self.emulator
            .clear_selection_if_viewport_changed(viewport_revision);
    }

    pub fn resize(&mut self, columns: usize, rows: usize) {
        self.emulator.resize(columns, rows);
    }

    /// Returns the active terminal grid's column and row counts.
    #[must_use]
    pub fn dimensions(&self) -> (usize, usize) {
        self.emulator.dimensions()
    }

    pub const fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.emulator.set_pixel_size(width, height);
    }

    #[must_use]
    pub fn frame_update(&mut self, force_full: bool) -> FrameUpdate {
        self.metrics.frame_requests = self.metrics.frame_requests.saturating_add(1);
        let update = self.emulator.frame_update(force_full);
        if update.has_damage() {
            self.metrics.damaged_frames = self.metrics.damaged_frames.saturating_add(1);
        }
        if update.full {
            self.metrics.full_frames = self.metrics.full_frames.saturating_add(1);
        }
        self.metrics.row_moves = self
            .metrics
            .row_moves
            .saturating_add(u64::try_from(update.row_moves.len()).unwrap_or(u64::MAX));
        self.metrics.rows_moved = self.metrics.rows_moved.saturating_add(
            update
                .row_moves
                .iter()
                .map(|movement| {
                    movement
                        .end_row
                        .saturating_sub(movement.start_row)
                        .saturating_sub(movement.count)
                })
                .map(|rows| u64::try_from(rows).unwrap_or(u64::MAX))
                .sum::<u64>(),
        );
        self.metrics.row_updates = self
            .metrics
            .row_updates
            .saturating_add(u64::try_from(update.row_updates.len()).unwrap_or(u64::MAX));
        let copied = update
            .row_updates
            .iter()
            .map(|row| row.cells.len())
            .sum::<usize>();
        self.metrics.cells_copied = self
            .metrics
            .cells_copied
            .saturating_add(u64::try_from(copied).unwrap_or(u64::MAX));
        update
    }

    #[must_use]
    pub const fn metrics(&self) -> TerminalMetrics {
        self.metrics
    }

    pub fn reset_metrics(&mut self) {
        self.metrics = TerminalMetrics::default();
    }

    /// Changes the main screen's retained scrollback and immediately releases rows above the new
    /// limit.
    pub fn set_scrollback_limit(&mut self, limit: usize) {
        let viewport_revision = self.emulator.viewport_revision();
        self.emulator.set_scrollback_limit(limit);
        self.emulator
            .clear_selection_if_viewport_changed(viewport_revision);
    }

    #[must_use]
    pub fn memory_stats(&self) -> TerminalMemoryStats {
        self.emulator.memory_stats()
    }

    #[must_use]
    pub fn drain_events(&mut self) -> Vec<TerminalEvent> {
        self.emulator.drain_events()
    }

    #[must_use]
    pub fn encode_key(&self, event: &KeyEvent) -> Vec<u8> {
        input::encode_key(
            event,
            self.emulator.keyboard_flags(),
            self.emulator.application_cursor(),
            self.emulator.application_keypad(),
            self.emulator.modify_other_keys(),
        )
    }

    /// Encodes a committed text/IME payload without inventing a physical key identity.
    #[must_use]
    pub fn encode_text(&self, text: &str) -> Vec<u8> {
        input::encode_text(text, self.emulator.keyboard_flags())
    }

    #[must_use]
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        input::encode_paste(text, self.emulator.bracketed_paste())
    }

    #[must_use]
    pub fn encode_mouse(&self, event: MouseEvent) -> Option<Vec<u8>> {
        let enabled = match self.emulator.mouse_tracking() {
            MouseTrackingMode::Disabled => false,
            MouseTrackingMode::Press => event.kind != MouseEventKind::Motion,
            MouseTrackingMode::ButtonMotion => {
                event.kind != MouseEventKind::Motion || event.button != MouseButton::None
            }
            MouseTrackingMode::AnyMotion => true,
        };
        enabled.then(|| {
            input::encode_mouse(
                event,
                self.emulator.sgr_mouse(),
                self.emulator.pixel_mouse(),
            )
        })
    }

    pub fn focus_changed(&mut self, focused: bool) -> Option<Vec<u8>> {
        self.emulator.focus_reporting().then(|| {
            if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            }
        })
    }

    pub fn scroll_display(&mut self, lines: isize) -> bool {
        self.emulator.scroll_display(lines)
    }

    pub fn scroll_to_bottom(&mut self) {
        self.emulator.scroll_to_bottom();
    }

    pub fn search_with_options(
        &mut self,
        query: &str,
        options: SearchOptions,
    ) -> Option<SearchMatch> {
        self.emulator.search_with_options(query, options)
    }

    pub fn reset_search(&mut self) -> bool {
        self.emulator.reset_search()
    }

    pub fn begin_selection(&mut self, point: SelectionPoint) -> bool {
        self.emulator.begin_selection(point)
    }

    pub fn begin_selection_with_mode(
        &mut self,
        point: SelectionPoint,
        mode: SelectionMode,
    ) -> bool {
        self.emulator.begin_selection_with_mode(point, mode)
    }

    pub fn update_selection(&mut self, point: SelectionPoint) -> bool {
        self.emulator.update_selection(point)
    }

    pub fn clear_selection(&mut self) -> bool {
        self.emulator.clear_selection()
    }

    #[must_use]
    pub fn selected_text(&self) -> Option<String> {
        self.emulator.selected_text()
    }

    #[must_use]
    pub fn hyperlink_at(&self, point: SelectionPoint) -> Option<Arc<String>> {
        self.emulator.hyperlink_at(point)
    }

    #[must_use]
    pub fn mouse_tracking_mode(&self) -> MouseTrackingMode {
        self.emulator.mouse_tracking()
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::{Terminal, TerminalConfig};

    #[test]
    fn snapshot_round_trip_preserves_terminal_buffer_and_modes() {
        let mut terminal = Terminal::new(TerminalConfig {
            columns: 12,
            rows: 3,
            scrollback_limit: 20,
        });
        terminal.feed(b"first\r\nsecond\r\n\x1b[31mred\x1b[0m\x1b[?2004h");
        terminal.resize(14, 4);
        terminal.set_pixel_size(140, 80);

        let snapshot = terminal.snapshot().expect("snapshot should encode");
        let mut restored = Terminal::from_snapshot(&snapshot).expect("snapshot should restore");

        assert_eq!(
            terminal.frame_update(true),
            restored.frame_update(true),
            "restored terminal should render the same cells and metadata"
        );
        assert_eq!(
            terminal.encode_paste("pasted"),
            restored.encode_paste("pasted"),
            "terminal protocol modes should survive the snapshot"
        );
    }

    #[test]
    fn snapshot_rejects_trailing_data() {
        let terminal = Terminal::new(TerminalConfig::default());
        let mut snapshot = terminal.snapshot().expect("snapshot should encode");
        snapshot.push(0);
        assert!(Terminal::from_snapshot(&snapshot).is_err());
    }
}
