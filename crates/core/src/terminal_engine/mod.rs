//! Termy's independent terminal engine.
//!
//! The parser, screen storage and protocol state are owned here. Parsing and
//! borrowed screen reads need neither a renderer nor a PTY, and do not depend on
//! Alacritty or the legacy experimental engine. See `README.md` for migration
//! status and the remaining integration gates.

mod dispatch;
mod graphics;
mod grid;
mod hyperlinks;
pub mod media;
mod parser;
mod queries;
pub(crate) mod snapshot;
mod sync;
#[cfg(feature = "pty")]
pub(crate) mod transport;
mod types;
#[cfg(test)]
mod unicode_tests;

use std::collections::VecDeque;
use web_time::Instant;

pub use types::{
    Cell, CellExtra, Color, Cursor, CursorShape, Damage, DirtySpan, Hyperlink, Line, Size, Style,
    UnderlineStyle, ViewportScroll,
};

pub use snapshot::{MIN_SNAPSHOT_VERSION, SNAPSHOT_VERSION, SnapshotError, snapshot_version};

/// The default scrollback byte limit, 16 MiB; see [`Options::scrollback_bytes`].
pub const DEFAULT_SCROLLBACK_BYTES: usize = grid::DEFAULT_HISTORY_BYTES;
/// The default (and largest) OSC and DCS payload, 64 KiB.
pub const MAX_STRING_PAYLOAD_BYTES: usize = parser::MAX_STRING_BYTES;
/// The default (and largest) OSC 52 clipboard payload, 8 MiB.
pub const MAX_CLIPBOARD_PAYLOAD_BYTES: usize = parser::MAX_CLIPBOARD_OSC_BYTES;

use dispatch::State;
use parser::Parser;
use sync::SynchronizedUpdate;

use crate::kitty_graphics::{KittyPreparedUpload, KittyUpload};

const MAX_EVENTS: usize = 1024;
const MAX_REPLY_BYTES: usize = 64 * 1024;
const PARSE_CHUNK: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// History rows to keep. `usize::MAX` leaves only `scrollback_bytes`.
    pub scrollback_history: usize,
    /// Bytes history may hold, whichever of the two limits binds first. Each
    /// row is charged its cells as packed with room for the whole row (see
    /// the module README) and the cell metadata it references, whether it is
    /// stored dense or packed, so engines fed the same bytes evict the same
    /// rows however their feeds were split and whenever they compacted.
    pub scrollback_bytes: usize,
    /// OSC and DCS payloads longer than this are dropped whole. Clamped to
    /// `1..=`[`MAX_STRING_PAYLOAD_BYTES`].
    pub max_string_bytes: usize,
    /// The same for OSC 52 (clipboard) payloads. Clamped to
    /// `1..=`[`MAX_CLIPBOARD_PAYLOAD_BYTES`].
    pub max_clipboard_bytes: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            scrollback_history: 1000,
            scrollback_bytes: DEFAULT_SCROLLBACK_BYTES,
            max_string_bytes: MAX_STRING_PAYLOAD_BYTES,
            max_clipboard_bytes: MAX_CLIPBOARD_PAYLOAD_BYTES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Bell,
    Title(String),
    ResetTitle,
    WorkingDirectory(String),
    ShellIntegration(String),
    Progress(crate::ProgressState),
    Clipboard { selection: String, data: String },
    KittyClipboard(crate::KittyClipboardOsc),
    KittyClipboardControl(crate::KittyClipboardControl),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseTracking {
    #[default]
    None,
    Press,
    Click,
    Drag,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseEncoding {
    #[default]
    Default,
    Utf8,
    Sgr,
    /// SGR with pixel coordinates (`?1016`); only recognized after
    /// [`Engine::set_pixel_mouse_reports`].
    SgrPixels,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub mouse_tracking: MouseTracking,
    pub mouse_encoding: MouseEncoding,
    /// `?1007`: on the alternate screen, hosts send wheel motion as cursor
    /// keys when no mouse tracking is active.
    pub alternate_scroll: bool,
    pub kitty_keyboard: u8,
    pub synchronized_update: bool,
    pub clipboard_paste_events: bool,
    /// DECSCNM (`?5`): renderers draw the screen with the default
    /// foreground and background swapped. Toggling it bumps
    /// [`Engine::palette_revision`].
    pub reverse_video: bool,
}

/// Reusable, single-owner parser and grid. The runtime controls synchronization;
/// the hot parsing path performs no locking and ordinary screen reads borrow.
pub struct Engine {
    parser: Parser,
    synchronized_update: SynchronizedUpdate,
    state: State,
    generation: u64,
}

impl Engine {
    pub fn new(size: Size, options: Options) -> Self {
        let mut parser = Parser::with_apc_limit(parser::MAX_APC_BYTES);
        parser.set_string_limits(options.max_string_bytes, options.max_clipboard_bytes);
        Self {
            parser,
            synchronized_update: SynchronizedUpdate::default(),
            state: State::new(size, options),
            generation: 0,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.feed_impl(bytes, None, false);
    }

    /// Like [`Self::feed`], but stops right after a kitty image upload
    /// completes, without decoding it, and returns the bytes consumed. The
    /// caller decodes [`Self::take_deferred_upload`] (e.g. without holding a
    /// lock), passes the result to [`Self::commit_upload`], then feeds the
    /// rest. Uploads inside a synchronized update are still applied inline.
    ///
    /// It also stops right after a clipboard request (OSC 52 query or kitty
    /// OSC 5522 packet): replies drained then answer output before the
    /// request, whose own reply the host may give later.
    pub(crate) fn feed_deferring_uploads(&mut self, bytes: &[u8]) -> usize {
        self.feed_impl(bytes, None, true)
    }

    pub(crate) fn has_deferred_upload(&self) -> bool {
        self.state.has_deferred_upload()
    }

    /// True after [`Self::feed_deferring_uploads`] stopped early because
    /// kitty animation edits did enough pixel work for one lock hold. The
    /// caller should release the engine lock before feeding the rest.
    pub(crate) fn animation_budget_spent(&self) -> bool {
        self.state.animation_budget_spent()
    }

    pub(crate) fn take_deferred_upload(&mut self) -> Option<KittyUpload> {
        self.state.take_deferred_upload()
    }

    pub(crate) fn commit_upload(&mut self, prepared: KittyPreparedUpload) {
        // The caller may have read history while the upload decoded, and the
        // placement's cursor advance can scroll (and evict) history.
        self.state.grid.release_history_read_cache();
        self.state.commit_upload(prepared);
        self.state.flush_graphics_effects();
        self.generation = self.generation.wrapping_add(1);
    }

    #[cfg(test)]
    fn feed_at(&mut self, bytes: &[u8], now: Instant) {
        self.feed_impl(bytes, Some(now), false);
    }

    /// Only synchronized updates need the time, and reading the clock would
    /// dominate small PTY reads, so `now` is read when first needed.
    #[inline]
    fn feed_impl(&mut self, bytes: &[u8], now: Option<Instant>, defer_uploads: bool) -> usize {
        if let [byte] = *bytes
            && self.feed_ascii_cell(byte)
        {
            return 1;
        }
        self.feed_parsed(bytes, now, defer_uploads)
    }

    /// `feed_impl` through the parser.
    #[inline(never)]
    fn feed_parsed(&mut self, bytes: &[u8], now: Option<Instant>, defer_uploads: bool) -> usize {
        // Most feeds are one parse pass with nothing deferred or buffered.
        if bytes.len() <= PARSE_CHUNK
            && !self.state.has_deferred_upload()
            && self.synchronized_update_deadline().is_none()
        {
            self.state.reset_animation_work();
            self.state.grid.prepare_output(bytes.len());
            let consumed = self.parse_pass(bytes, defer_uploads, &mut || {
                now.unwrap_or_else(Instant::now)
            });
            if consumed == bytes.len()
                || self.state.has_deferred_upload()
                || (defer_uploads && self.state.animation_budget_spent())
                || self.state.clipboard_pause
            {
                return consumed;
            }
            // A synchronized update began: the general path buffers the rest.
            return consumed + self.feed_impl_general(&bytes[consumed..], now, defer_uploads);
        }
        self.feed_impl_general(bytes, now, defer_uploads)
    }

    /// Typed and echoed text arrives a byte at a time. Feed one printable
    /// byte that the grid writes in place with exactly the effects of
    /// `parse_pass`, minus its setup; returns false, having changed nothing,
    /// when that pass would do more (an upload or synchronized update is
    /// pending, a sequence is open, or the write wraps, inserts or repairs).
    #[inline]
    fn feed_ascii_cell(&mut self, byte: u8) -> bool {
        if !(0x20..=0x7e).contains(&byte)
            || !self.parser.at_ground()
            || self.state.has_deferred_upload()
            || self.state.modes.synchronized_update
            || self.synchronized_update_deadline().is_some()
            || !self.state.print_ascii_cell(byte)
        {
            return false;
        }
        // The rest of `feed_impl` and `parse_pass` for this byte: printing
        // leaves no pending clear, upload deferral or synchronized update.
        self.state.reset_animation_work();
        self.state.grid.prepare_output(1);
        self.state.flush_graphics_effects();
        self.generation = self.generation.wrapping_add(1);
        true
    }

    /// Parse `bytes` (at most one chunk) through the grid, as one iteration
    /// of `feed_impl_general`'s loop outside a synchronized update.
    #[inline]
    fn parse_pass(
        &mut self,
        bytes: &[u8],
        defer_uploads: bool,
        now: &mut impl FnMut() -> Instant,
    ) -> usize {
        self.state.set_defer_uploads(defer_uploads);
        self.state.clipboard_pause = false;
        let consumed = self.parser.advance(&mut self.state, bytes);
        // Nothing outside parsing may see a partly blanked row.
        self.state.grid.flush_pending_clear();
        self.state.set_defer_uploads(false);
        self.state.flush_graphics_effects();
        self.generation = self.generation.wrapping_add(1);
        if self.state.modes.synchronized_update {
            self.synchronized_update
                .begin(now(), self.state.saved_private_mode(2026));
        }
        consumed
    }

    #[inline(never)]
    fn feed_impl_general(
        &mut self,
        bytes: &[u8],
        now: Option<Instant>,
        defer_uploads: bool,
    ) -> usize {
        let mut now = now;
        let mut now = move || *now.get_or_insert_with(Instant::now);
        // An upload nobody took must not keep pausing the parser.
        if self.state.has_deferred_upload()
            && let Some(upload) = self.state.take_deferred_upload()
        {
            self.commit_upload(upload.prepare());
        }
        self.state.reset_animation_work();
        self.state.grid.prepare_output(bytes.len());
        if self
            .synchronized_update_deadline()
            .is_some_and(|deadline| deadline <= now())
        {
            self.stop_synchronized_update();
        }
        let mut offset = 0;
        while offset < bytes.len() {
            if self.synchronized_update_deadline().is_some() {
                let buffered = self.synchronized_update.push(&bytes[offset..], now());
                offset += buffered.consumed;
                if buffered.commit {
                    self.stop_synchronized_update();
                }
            } else {
                let end = offset.saturating_add(PARSE_CHUNK).min(bytes.len());
                offset += self.parse_pass(&bytes[offset..end], defer_uploads, &mut now);
                if self.state.has_deferred_upload()
                    || (defer_uploads && self.state.animation_budget_spent())
                    || self.state.clipboard_pause
                {
                    break;
                }
            }
        }
        offset
    }

    /// The runtime can schedule a watchdog without polling or copying a frame.
    /// Compare its captured deadline with this value before firing a stale task.
    pub fn synchronized_update_deadline(&self) -> Option<Instant> {
        self.synchronized_update.deadline()
    }

    /// Commit pending output atomically through the existing parser and grid.
    /// Returns false when there is no pending batch (including stale watchdogs).
    pub fn stop_synchronized_update(&mut self) -> bool {
        self.state.grid.release_history_read_cache();
        let Some(bytes) = self.synchronized_update.take_buffer() else {
            return false;
        };
        for chunk in bytes.chunks(PARSE_CHUNK) {
            self.parser.advance_uninterrupted(&mut self.state, chunk);
            self.state.grid.flush_pending_clear();
            self.state.flush_graphics_effects();
        }
        self.state.modes.synchronized_update = false;
        self.state.flush_graphics_effects();
        self.synchronized_update.recycle_buffer(bytes);
        self.generation = self.generation.wrapping_add(1);
        true
    }

    /// Current OSC 7501 records, oldest update first, with inherited apps resolved.
    pub fn program_status(&self) -> Vec<crate::ProgramStatusRecord> {
        self.state.program_status.snapshot()
    }

    pub(crate) fn has_program_status_changes(&self) -> bool {
        self.state.program_status.has_changes()
    }

    /// Coalesced status snapshot, independent of the bounded transient event queue.
    /// An empty snapshot means that the final record was cleared.
    pub fn take_program_status(&mut self) -> Option<Vec<crate::ProgramStatusRecord>> {
        self.state.program_status.take_changed()
    }

    /// Hosts must call this when the process attached to this terminal exits.
    pub fn process_exited(&mut self) {
        self.stop_synchronized_update();
        self.state.program_status.finish();
    }

    pub fn size(&self) -> Size {
        self.state.grid.size()
    }
    pub fn cursor(&self) -> Cursor {
        self.state.grid.cursor
    }
    pub fn modes(&self) -> Modes {
        self.state.modes
    }

    /// DECRQM state of an ANSI (`private == false`) or DEC private mode:
    /// `None` when the engine does not recognize the mode.
    pub fn mode(&self, private: bool, mode: u16) -> Option<bool> {
        if private && mode == 2027 {
            // Grapheme clustering is permanently enabled.
            return Some(true);
        }
        self.state.mode_state(private, mode)
    }

    /// The window title from OSC 0/2 (and XTWINOPS title restores).
    pub fn title(&self) -> &str {
        self.state.title()
    }

    /// The last working directory reported with OSC 7 or OSC 9;9.
    pub fn working_directory(&self) -> Option<&str> {
        self.state.working_directory()
    }

    /// The configured scrollback, in rows. History also stays within
    /// [`Self::scrollback_byte_limit`]; see `README.md`.
    pub fn scrollback_limit(&self) -> usize {
        self.state.grid.history_limit()
    }

    /// The configured scrollback byte limit.
    pub fn scrollback_byte_limit(&self) -> usize {
        self.state.grid.history_byte_limit()
    }

    /// The options in effect (payload limits after clamping).
    pub fn options(&self) -> Options {
        let (max_string_bytes, max_clipboard_bytes) = self.parser.string_limits();
        Options {
            scrollback_history: self.scrollback_limit(),
            scrollback_bytes: self.scrollback_byte_limit(),
            max_string_bytes,
            max_clipboard_bytes,
        }
    }

    /// Rows output has scrolled into history so far. History row `index`
    /// (0 the oldest) has the stable number `scrolled_into_history() -
    /// history_size() + index`, and live row `row` has
    /// `scrolled_into_history() + row`, so a host can anchor a selection to
    /// content while output scrolls and old history is evicted. Resizing
    /// reflows rows without updating this count; hosts reset anchors then.
    pub fn scrolled_into_history(&self) -> u64 {
        self.state.grid.scrolled_into_history()
    }

    /// Drop queued protocol replies without touching other state. Replica
    /// engines, whose replies the authoritative terminal already sent, call
    /// this after each feed.
    pub fn discard_replies(&mut self) {
        self.state.replies.clear();
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Compact cold scrollback after an output burst. This preserves all cells
    /// and damage; borrowed row views remain available on demand. The native
    /// runtime schedules bounded steps after 250 ms without output, or at most
    /// 2 s after history first needs compaction.
    pub fn compact_history(&mut self) {
        self.state.grid.compact_history(usize::MAX);
    }

    pub(crate) fn take_history_activity(&mut self) -> bool {
        std::mem::take(&mut self.state.grid.history_activity)
    }

    pub(crate) fn needs_history_compaction(&self) -> bool {
        self.state.grid.needs_compaction()
    }
    /// `quiet` steps follow an idle period, so later scrolled rows are packed
    /// as they enter history. Steps forced during output only drain the queue.
    pub(crate) fn compact_history_step(&mut self, rows: usize, quiet: bool) {
        if quiet {
            self.state.grid.compact_history(rows);
        } else {
            self.state.grid.compact_pending_history(rows);
        }
    }

    pub fn history_size(&self) -> usize {
        self.state.grid.history_size()
    }
    pub fn display_offset(&self) -> usize {
        self.state.grid.display_offset()
    }
    pub fn alternate_screen(&self) -> bool {
        self.state.grid.alternate_screen()
    }

    pub fn set_options(&mut self, options: Options) {
        self.parser
            .set_string_limits(options.max_string_bytes, options.max_clipboard_bytes);
        self.state
            .grid
            .set_history_limits(options.scrollback_history, options.scrollback_bytes);
        self.state.flush_graphics_effects();
        self.generation = self.generation.wrapping_add(1);
    }

    /// The lifetime of the line prevents mutation while the renderer reads it.
    pub fn viewport_row(&self, row: usize) -> Option<Line<'_>> {
        self.state.grid.visible_row_line(row)
    }

    /// Visit a viewport row without retaining a dense copy of cold history.
    /// Renderers reuse `scratch` across rows and frames.
    pub(crate) fn with_viewport_row<T>(
        &self,
        row: usize,
        scratch: &mut Vec<Cell>,
        read: impl FnOnce(Line<'_>) -> T,
    ) -> Option<T> {
        self.state
            .grid
            .visible_row(row)
            .map(|row| row.with_line(scratch, read))
    }

    /// [`Self::with_viewport_row`], also passing whether the row wraps
    /// ([`Self::viewport_row_wrapped`]), where its erase blanks start (at
    /// most its length; past it for no blanks) and their background. Every
    /// cell from there on is a space in the default style on that
    /// background, so a renderer can fill instead of converting.
    pub(crate) fn with_viewport_row_blanks<T>(
        &self,
        row: usize,
        scratch: &mut Vec<Cell>,
        read: impl FnOnce(&[Cell], bool, usize, Color) -> T,
    ) -> Option<T> {
        self.state.grid.visible_row(row).map(|row| {
            let (blanks, background) = row.blank_suffix();
            let wrapped = row.wrapped;
            row.with_cells(scratch, |cells| read(cells, wrapped, blanks, background))
        })
    }

    pub fn viewport_row_wrapped(&self, row: usize) -> bool {
        self.state
            .grid
            .visible_row(row)
            .is_some_and(|row| row.wrapped)
    }

    /// Lines before the live screen are negative, with -1 the newest history row.
    /// A cold history row is expanded on demand. Expanded history is released
    /// on the next output, resize, or scroll; the borrowed slice remains valid
    /// until that exclusive mutation.
    pub fn line(&self, line: i32) -> Option<Line<'_>> {
        self.state.grid.row_line(line)
    }

    /// Visit a line without retaining a dense copy of cold history. Scratch
    /// capacity is reused across calls, including full-buffer search/copy.
    pub(crate) fn with_line<T>(
        &self,
        line: i32,
        scratch: &mut Vec<Cell>,
        read: impl FnOnce(Line<'_>) -> T,
    ) -> Option<T> {
        self.state
            .grid
            .row(line)
            .map(|row| row.with_line(scratch, read))
    }

    pub fn line_wrapped(&self, line: i32) -> bool {
        self.state.grid.row(line).is_some_and(|row| row.wrapped)
    }

    pub fn set_query_colors(&mut self, colors: crate::TerminalQueryColors) {
        self.state.query_colors = colors;
    }

    pub fn set_default_cursor_shape(&mut self, shape: CursorShape) {
        self.state.default_cursor_shape = shape;
        if !self.state.cursor_shape_overridden {
            let old = self.state.grid.cursor;
            self.state.grid.cursor.shape = shape;
            self.state.grid.cursor_changed(old);
        }
    }

    /// Recognize `?1016` (SGR pixel mouse reports). Enable it only when the
    /// host reports pointer positions in pixels; otherwise the mode stays
    /// unrecognized, as DECRQM reports. Engines that must agree (a server
    /// and its replicas) need the same setting; snapshots carry it.
    pub fn set_pixel_mouse_reports(&mut self, enabled: bool) {
        self.state.pixel_mouse_reports = enabled;
        if !enabled && self.state.modes.mouse_encoding == MouseEncoding::SgrPixels {
            self.state.modes.mouse_encoding = MouseEncoding::Default;
        }
    }

    pub fn set_cell_pixels(&mut self, width: f32, height: f32) {
        let clamp = |value: f32| {
            if value.is_finite() {
                value.round().clamp(1.0, 65535.0) as u16
            } else {
                1
            }
        };
        self.state.cell_pixels = (clamp(width), clamp(height));
        self.state.resize_graphics();
    }

    pub fn take_damage(&mut self) -> Damage {
        self.state.grid.take_damage()
    }

    /// Replay the ordered row rotations, then patch the damaged spans from the
    /// current viewport. Cell-only consumers can continue using `take_damage`.
    pub fn take_render_damage(&mut self) -> (Damage, Vec<ViewportScroll>) {
        self.state.grid.take_render_damage()
    }

    /// Resizing to the current size changes nothing: it neither commits a
    /// synchronized update nor closes the grapheme being printed.
    pub fn resize(&mut self, size: Size) {
        if size.clamped() == self.size() {
            return;
        }
        self.stop_synchronized_update();
        self.state.grid.resize(size);
        self.state.resize_graphics();
        self.state.flush_graphics_effects();
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn scroll_display(&mut self, delta: i32) -> bool {
        let changed = self.state.grid.scroll_display(delta);
        if changed {
            self.state.flush_graphics_effects();
            self.generation = self.generation.wrapping_add(1);
        }
        changed
    }

    pub fn clear_scrollback(&mut self) {
        self.state.grid.clear_scrollback();
        self.state.flush_graphics_effects();
        self.generation = self.generation.wrapping_add(1);
    }

    /// Whether `pop_event` or `drain_replies` has anything to hand out.
    #[inline]
    pub(crate) fn has_output(&self) -> bool {
        !self.state.events.is_empty() || !self.state.replies.is_empty()
    }

    pub fn pop_event(&mut self) -> Option<Event> {
        self.state.events.pop_front()
    }

    /// Draining into a caller-owned buffer allows the PTY writer to reuse it.
    pub fn drain_replies(&mut self, output: &mut Vec<u8>) {
        output.append(&mut self.state.replies);
    }

    pub fn palette(&self) -> &[Option<Color>; 256] {
        &self.state.palette
    }
    pub fn foreground(&self) -> Option<Color> {
        self.state.foreground
    }
    pub fn background(&self) -> Option<Color> {
        self.state.background
    }
    pub fn cursor_color(&self) -> Option<Color> {
        self.state.cursor_color
    }
    pub fn palette_revision(&self) -> u64 {
        self.state.palette_revision
    }

    pub fn graphics_revision(&self) -> u64 {
        self.state.graphics.revision
    }

    /// Advance image animation deadlines without materializing a frame.
    pub fn poll_graphics_revision(&mut self) -> u64 {
        self.state.poll_graphics_revision()
    }

    pub fn graphics_snapshot(&mut self) -> (u64, Vec<crate::KittyGraphicsRenderPlacement>) {
        self.state.graphics_snapshot()
    }

    pub fn graphics_placements(&mut self) -> Vec<crate::KittyGraphicsRenderPlacement> {
        self.graphics_snapshot().1
    }

    /// Reports bounded output queues reaching capacity. The runtime can drain
    /// them between reads; hostile output cannot grow these queues forever.
    pub fn dropped_events(&self) -> u64 {
        self.state.dropped_events
    }
    pub fn dropped_reply_bytes(&self) -> u64 {
        self.state.dropped_reply_bytes
    }
}

fn enqueue(events: &mut VecDeque<Event>, dropped: &mut u64, paste_events: bool, event: Event) {
    if events.len() == MAX_EVENTS {
        if matches!(
            event,
            Event::KittyClipboard(_) | Event::KittyClipboardControl(_)
        ) {
            // Losing a write chunk or a reset must not leave a host with an
            // incomplete clipboard transaction or a stale permission grant.
            // Compaction happens only at capacity, amortizing the cleanup.
            *dropped = dropped.saturating_add(events.len() as u64);
            events.clear();
            events.push_back(Event::KittyClipboardControl(
                crate::KittyClipboardControl::Reset,
            ));
            events.push_back(Event::KittyClipboardControl(
                crate::KittyClipboardControl::Set(paste_events),
            ));
        } else {
            *dropped = dropped.saturating_add(1);
            return;
        }
    }
    events.push_back(event);
}

#[cfg(test)]
mod tests;
