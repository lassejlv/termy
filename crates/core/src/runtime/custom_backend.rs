//! Adapts the independent parser/grid and native transport to Termy's public
//! terminal API. Renderer callbacks borrow one coherent engine state.

use super::{
    ChildEnvironment, MAX_TERMINAL_SCROLLBACK_HISTORY, TabTitleShellIntegration,
    TerminalCursorState, TerminalCursorStyle, TerminalDamageSnapshot, TerminalDirtySpan,
    TerminalEvent, TerminalLaunch, TerminalOptions, TerminalRuntimeConfig, TerminalSize,
    TerminalWakeupNotifier, explicit_child_environment, resolve_launch_working_directory,
    resolve_terminal_launch_in, terminal_environment_overrides_in,
};
use crate::{
    DetectedLink, DetectedViewportLink, KittyClipboardControl, KittyClipboardHostState,
    KittyClipboardOsc, KittyGraphicsRenderPlacement, TerminalClipboardLocation,
    TerminalClipboardTarget, TerminalColor, TerminalKeyboardMode, TerminalMouseMode,
    TerminalPalette, TerminalQueryColors, TerminalRenderCell, TerminalRenderColor,
    TerminalRenderDamageSnapshot, TerminalRenderRead, TerminalRenderText, TerminalReplyHost,
    TerminalUnderlineStyle, TerminalViewportMetadata, TerminalViewportScroll,
    TerminalViewportScrollDirection, TermyCell, TermyColor, TermyFrame, TermyFrameUpdate,
    TermySearchMatch, TermySearchOptions, TermySharedSearchMatch,
    kitty_graphics::KittyPreparedUpload,
    protocol::{KittyClipboardStep, TerminalClipboardRequest},
    search::search_lines_shared,
    search_engine::SearchLineMapping,
    terminal_engine::transport::{PtySize, SpawnConfig, Transport},
    terminal_engine::{self as engine, Engine},
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as BASE64, STANDARD_NO_PAD},
};
use flume::Sender;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

mod deferred;
mod legacy;
mod lock;
mod maintenance;
mod replica;
mod viewport;
use legacy::LegacyCells;

const EVENT_BATCH: usize = 2048;
const MAX_EVENTS: usize = 65_536;
// Count limits alone allow multi-gigabyte backlogs of maximum-size OSC strings.
// Track retained heap capacity independently of the fixed-size queue slots.
const MAX_EVENT_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_PENDING_REPLIES: usize = 2 * 1024 * 1024;
const PARSE_BATCH: usize = 4096;
const HISTORY_COMPACTION_IDLE_DELAY: Duration = Duration::from_millis(250);
const HISTORY_COMPACTION_MAX_DELAY: Duration = Duration::from_secs(2);

pub(super) struct CustomBackend {
    shared: Arc<Shared>,
    transport: Option<Arc<Transport>>,
}

struct Shared {
    // Taken for every feed and read; see `lock::RawEngineLock`. The macOS
    // pthread mutex costs far more per call.
    state: lock::EngineMutex,
    lock_waiters: lock::LockWaiters,
    clipboard: Mutex<KittyClipboardHostState>,
    deferred_clipboard: Mutex<deferred::DeferredClipboard>,
    notifier: Option<TerminalWakeupNotifier>,
    wakeup_enabled: AtomicBool,
    wakeup_queued: AtomicBool,
    /// Set once after spawning; read on every PTY read, so it takes no lock.
    transport: OnceLock<Weak<Transport>>,
    sync_signal: Arc<SyncSignal>,
    sync_watchdog_started: AtomicBool,
    // Set when keys or a paste could not queue; drained as one event.
    input_dropped: AtomicBool,
}

#[derive(Default)]
struct SyncSignal {
    state: Mutex<SyncSignalState>,
    changed: Condvar,
}

#[derive(Default)]
struct SyncSignalState {
    deadline: Option<Instant>,
    shutdown: bool,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.sync_signal
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown = true;
        self.sync_signal.changed.notify_one();
    }
}

struct State {
    engine: Engine,
    size: TerminalSize,
    query_colors: TerminalQueryColors,
    default_cursor_style: TerminalCursorStyle,
    events: VecDeque<PendingEvent>,
    event_payload_bytes: usize,
    replies: Vec<u8>,
    generation: u64,
    palette_epoch: u64,
    force_full_damage: bool,
    last_damage_cursor: Option<TerminalCursorState>,
    history_deadline: Option<Instant>,
    // First output that left history uncompacted. Bounds how long steady
    // output can keep postponing the idle compaction deadline.
    history_pending_since: Option<Instant>,
    // Whether the pending deadline is the idle delay rather than the cap.
    history_quiet: bool,
    // A title event was queued since the last feed or commit woke the host.
    title_changed: bool,
    // False for replicas of another terminal: protocol replies are dropped.
    replies_enabled: bool,
    // The maintenance deadline last handed to the timer, or `None` when the
    // timer may hold a different one. Feeds skip the timer's lock while their
    // deadline is unchanged.
    published_maintenance: Option<Option<Instant>>,
    /// Queued events that write to the program in order (see
    /// `PendingEvent::is_ordered`).
    ordered_events: usize,
    /// Ordered events that `drain_events` took but has not written yet,
    /// most of them waiting for a deferred clipboard answer.
    ordered_in_flight: usize,
}

enum PendingEvent {
    Terminal(TerminalEvent),
    ClipboardLoad(String),
    Kitty(KittyClipboardOsc),
    KittyControl(KittyClipboardControl),
    KittyOverflow,
    /// Protocol replies to output after a clipboard request whose reply is
    /// still owed: they go out after it.
    Reply(Vec<u8>),
}

impl PendingEvent {
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Terminal(
                TerminalEvent::Title(text)
                | TerminalEvent::WorkingDirectory(text)
                | TerminalEvent::ClipboardStore(text),
            )
            | Self::ClipboardLoad(text) => text.capacity(),
            Self::Kitty(packet) => packet.payload_capacity(),
            Self::Reply(bytes) => bytes.capacity(),
            Self::Terminal(_) | Self::KittyControl(_) | Self::KittyOverflow => 0,
        }
    }

    /// Events whose output to the program keeps the order of the output
    /// that caused it: clipboard requests and the replies held behind them.
    fn is_ordered(&self) -> bool {
        matches!(
            self,
            Self::ClipboardLoad(_) | Self::Kitty(_) | Self::Reply(_)
        )
    }

    fn must_deliver(&self) -> bool {
        matches!(
            self,
            Self::Terminal(TerminalEvent::Exit)
                | Self::KittyOverflow
                | Self::KittyControl(KittyClipboardControl::Set(_) | KittyClipboardControl::Reset)
        )
    }
}

impl Shared {
    fn notify(&self) {
        if self.wakeup_enabled.load(Ordering::Acquire) {
            self.queue_wakeup();
        }
    }

    // Titles and program status drive tab chrome, which stays visible for
    // hidden tabs, so they wake the host even while render wakeups are suspended.
    fn notify_tab_chrome(&self) {
        self.queue_wakeup();
    }

    fn queue_wakeup(&self) {
        // Output arrives far faster than hosts drain wakeups, so the flag is
        // usually set already; a plain load avoids an atomic swap per feed.
        if !self.wakeup_queued.load(Ordering::Acquire)
            && !self.wakeup_queued.swap(true, Ordering::AcqRel)
        {
            crate::render_metrics::increment_runtime_wakeup_count();
            if let Some(notifier) = &self.notifier {
                notifier.notify();
            }
        }
    }
}

impl State {
    /// Compact after output has been quiet for a short delay, but never wait
    /// longer than the maximum delay, so a steady trickle still gets packed.
    fn defer_history_compaction(&mut self, now: Instant) {
        if !self.engine.needs_history_compaction() {
            self.history_deadline = None;
            self.history_pending_since = None;
            return;
        }
        let since = *self.history_pending_since.get_or_insert(now);
        let idle = now + HISTORY_COMPACTION_IDLE_DELAY;
        let cap = since + HISTORY_COMPACTION_MAX_DELAY;
        self.history_quiet = idle <= cap;
        self.history_deadline = Some(idle.min(cap));
    }

    #[inline]
    fn maintenance_deadline(&self) -> Option<Instant> {
        match (
            self.engine.synchronized_update_deadline(),
            self.history_deadline,
        ) {
            (Some(sync), Some(history)) => Some(sync.min(history)),
            (sync, history) => sync.or(history),
        }
    }

    fn new(size: TerminalSize, config: &TerminalRuntimeConfig) -> Self {
        let mut engine = Engine::new(
            engine_size(size.clamped()),
            engine::Options {
                scrollback_history: config
                    .scrollback_history
                    .min(MAX_TERMINAL_SCROLLBACK_HISTORY),
                ..engine::Options::default()
            },
        );
        engine.set_default_cursor_shape(cursor_shape(config.default_cursor_style));
        engine.set_query_colors(config.query_colors);
        engine.set_cell_pixels(size.cell_width, size.cell_height);
        let actual = engine.size();
        Self {
            engine,
            size: TerminalSize {
                cols: actual.cols as u16,
                rows: actual.rows as u16,
                ..size.clamped()
            },
            query_colors: config.query_colors,
            default_cursor_style: config.default_cursor_style,
            events: VecDeque::new(),
            event_payload_bytes: 0,
            replies: Vec::new(),
            generation: 0,
            palette_epoch: 0,
            force_full_damage: true,
            last_damage_cursor: None,
            history_deadline: None,
            history_pending_since: None,
            history_quiet: true,
            title_changed: false,
            replies_enabled: true,
            published_maintenance: None,
            ordered_events: 0,
            ordered_in_flight: 0,
        }
    }

    /// Sends `replies` to `out`, or queues them behind the clipboard
    /// requests whose replies are still owed, so the program reads every
    /// reply in the order of its requests (`OSC 52 ?` then `CSI c`).
    fn route_replies(&mut self, mut replies: Vec<u8>, out: &mut Vec<u8>) {
        if replies.is_empty() || !self.replies_enabled {
            return;
        }
        if self.ordered_events > 0 || self.ordered_in_flight > 0 {
            self.queue(PendingEvent::Reply(replies));
        } else {
            out.append(&mut replies);
        }
    }

    /// The engine's replies, routed by `route_replies`, then its events. A
    /// deferring feed pauses right after a clipboard request, so the replies
    /// drained with it answer earlier output.
    fn take_engine_output(&mut self) -> Vec<u8> {
        let mut drained = Vec::new();
        self.engine.drain_replies(&mut drained);
        let mut replies = Vec::new();
        self.route_replies(drained, &mut replies);
        while let Some(event) = self.engine.pop_event() {
            self.engine_event(event);
        }
        replies
    }

    fn queue(&mut self, event: PendingEvent) {
        let bytes = event.payload_bytes();
        if bytes > MAX_EVENT_PAYLOAD_BYTES.saturating_sub(self.event_payload_bytes) {
            self.discard_event(event);
            return;
        }
        if self.events.len() == MAX_EVENTS {
            if !event.must_deliver() {
                self.discard_event(event);
                return;
            }
            self.compact_saturated_events();
        }
        self.event_payload_bytes += bytes;
        self.ordered_events += usize::from(event.is_ordered());
        self.events.push_back(event);
    }

    fn discard_event(&mut self, event: PendingEvent) {
        if matches!(event, PendingEvent::Kitty(_)) {
            // A missing chunk must not allow a later terminator to commit a
            // truncated clipboard write. Keep this abort ordered with packets.
            self.queue(PendingEvent::KittyOverflow);
        }
    }

    fn pop_event(&mut self) -> Option<PendingEvent> {
        let event = self.events.pop_front()?;
        self.event_payload_bytes -= event.payload_bytes();
        self.ordered_events -= usize::from(event.is_ordered());
        Some(event)
    }

    // A critical event must survive even a queue filled entirely with controls.
    // Discard the saturated ordinary backlog and retain cumulative clipboard
    // invalidation, final paste mode, and process exit. All pending clipboard
    // packets are discarded together, so compression cannot move them across a
    // permission reset. This O(n) pass occurs only after MAX_EVENTS enqueues.
    fn compact_saturated_events(&mut self) {
        let mut reset = false;
        let mut revoke_grants = false;
        let mut clipboard_overflow = false;
        let mut paste_mode = None;
        let mut exit = false;
        while let Some(event) = self.pop_event() {
            match event {
                PendingEvent::KittyControl(KittyClipboardControl::Reset) => {
                    reset = true;
                    paste_mode = Some(false);
                }
                PendingEvent::KittyControl(KittyClipboardControl::Set(enabled)) => {
                    revoke_grants |= !enabled;
                    paste_mode = Some(enabled);
                }
                PendingEvent::Terminal(TerminalEvent::Exit) => exit = true,
                PendingEvent::Kitty(_) | PendingEvent::KittyOverflow => clipboard_overflow = true,
                _ => {}
            }
        }
        if reset {
            self.queue(PendingEvent::KittyControl(KittyClipboardControl::Reset));
        } else if clipboard_overflow {
            self.queue(PendingEvent::KittyOverflow);
        }
        if revoke_grants && !reset {
            self.queue(PendingEvent::KittyControl(KittyClipboardControl::Set(
                false,
            )));
        }
        if paste_mode == Some(true) {
            self.queue(PendingEvent::KittyControl(KittyClipboardControl::Set(true)));
        }
        if exit {
            self.queue(PendingEvent::Terminal(TerminalEvent::Exit));
        }
    }

    /// Returns replies and the number of bytes consumed. Outside hydration,
    /// parsing stops after a completed image upload, which the caller decodes
    /// and passes to `commit_upload` before feeding the rest.
    fn feed(&mut self, bytes: &[u8], hydrate: bool) -> (Vec<u8>, usize) {
        let before_generation = self.engine.generation();
        let mut replies = Vec::new();
        let mut consumed = 0;
        while consumed < bytes.len() {
            let chunk = &bytes[consumed..bytes.len().min(consumed + PARSE_BATCH)];
            if hydrate {
                self.engine.feed(chunk);
                consumed += chunk.len();
            } else {
                consumed += self.engine.feed_deferring_uploads(chunk);
            }
            if hydrate {
                while self.engine.pop_event().is_some() {}
                self.engine.drain_replies(&mut replies);
                replies.clear();
            } else {
                replies.append(&mut self.take_engine_output());
            }
            // Heavy kitty animation edits pause the parser too, so the lock
            // is released between them instead of after the whole read.
            if self.engine.has_deferred_upload()
                || (!hydrate && self.engine.animation_budget_spent())
            {
                break;
            }
        }
        if hydrate {
            self.engine.stop_synchronized_update();
            while self.engine.pop_event().is_some() {}
            self.engine.drain_replies(&mut replies);
            replies.clear();
        }
        if self.engine.generation() != before_generation {
            self.generation = self.generation.wrapping_add(1);
        }
        (replies, consumed)
    }

    /// Queue the engine's events and buffer its replies, as `feed` does for
    /// a display terminal.
    #[inline(never)]
    fn drain_engine_output(&mut self) {
        let replies = self.take_engine_output();
        self.append_replies(&replies);
    }

    fn commit_upload(&mut self, prepared: KittyPreparedUpload) -> Vec<u8> {
        self.engine.commit_upload(prepared);
        self.generation = self.generation.wrapping_add(1);
        self.take_engine_output()
    }

    fn take_tab_chrome_changed(&mut self) -> bool {
        std::mem::take(&mut self.title_changed) || self.engine.has_program_status_changes()
    }

    fn engine_event(&mut self, event: engine::Event) {
        let event = match event {
            engine::Event::KittyClipboard(packet) => {
                self.queue(PendingEvent::Kitty(packet));
                return;
            }
            engine::Event::KittyClipboardControl(control) => {
                self.queue(PendingEvent::KittyControl(control));
                return;
            }
            engine::Event::Bell => TerminalEvent::Bell,
            engine::Event::Title(title) => {
                self.title_changed = true;
                TerminalEvent::Title(title)
            }
            engine::Event::ResetTitle => {
                self.title_changed = true;
                TerminalEvent::ResetTitle
            }
            engine::Event::Progress(progress) => TerminalEvent::Progress(progress),
            engine::Event::WorkingDirectory(path) => TerminalEvent::WorkingDirectory(path),
            engine::Event::ShellIntegration(value) => {
                let mut parts = value.split(';');
                match parts.next() {
                    Some("A") => TerminalEvent::ShellPromptStart,
                    Some("B") => TerminalEvent::ShellCommandStart,
                    Some("C") => TerminalEvent::ShellCommandExecuting,
                    Some("D") => TerminalEvent::ShellCommandFinished(
                        parts.next().and_then(|code| code.parse().ok()),
                    ),
                    _ => return,
                }
            }
            engine::Event::Clipboard { selection, data } => {
                if data == "?" {
                    self.queue(PendingEvent::ClipboardLoad(selection));
                    return;
                }
                let Ok(bytes) = STANDARD_NO_PAD
                    .decode(data.trim_end_matches('=').as_bytes())
                    .or_else(|_| BASE64.decode(&data))
                else {
                    return;
                };
                let Ok(text) = String::from_utf8(bytes) else {
                    return;
                };
                TerminalEvent::ClipboardStore(text)
            }
        };
        self.queue(PendingEvent::Terminal(event));
    }

    fn append_replies(&mut self, bytes: &[u8]) {
        if !self.replies_enabled {
            return;
        }
        let available = MAX_PENDING_REPLIES.saturating_sub(self.replies.len());
        if bytes.len() <= available {
            self.replies.extend_from_slice(bytes);
        } else {
            log::warn!("terminal protocol reply queue is full");
        }
    }

    fn cursor(&self) -> Option<TerminalCursorState> {
        let cursor = self.engine.cursor();
        let row = cursor.row.saturating_add(self.engine.display_offset());
        (cursor.visible && row < self.engine.size().rows).then_some(TerminalCursorState {
            row,
            col: cursor.col,
            style: match cursor.shape {
                engine::CursorShape::Block => TerminalCursorStyle::Block,
                engine::CursorShape::Beam | engine::CursorShape::Underline => {
                    TerminalCursorStyle::Line
                }
            },
        })
    }

    fn metadata(&self) -> TerminalViewportMetadata {
        TerminalViewportMetadata {
            cols: self.size.cols,
            rows: self.size.rows,
            cursor: self.cursor(),
            display_offset: self.engine.display_offset(),
            history_size: self.engine.history_size(),
            palette_revision: self
                .engine
                .palette_revision()
                .wrapping_add(self.palette_epoch),
            generation: self.generation,
        }
    }

    fn palette(&self) -> TerminalPalette {
        let mut foreground = self.engine.foreground().and_then(rgb);
        let mut background = self.engine.background().and_then(rgb);
        // DECSCNM swaps the default colors. Resolve both first (program
        // override, else the host's configured colors) so the swap is total.
        if self.engine.modes().reverse_video {
            let resolved_foreground = foreground.unwrap_or(self.query_colors.foreground);
            let resolved_background = background.unwrap_or(self.query_colors.background);
            foreground = Some(resolved_background);
            background = Some(resolved_foreground);
        }
        TerminalPalette {
            indexed: std::array::from_fn(|index| self.engine.palette()[index].and_then(rgb)),
            foreground,
            background,
            cursor: self.engine.cursor_color().and_then(rgb),
            revision: self
                .engine
                .palette_revision()
                .wrapping_add(self.palette_epoch),
        }
    }

    fn take_damage(&mut self, force_full: bool) -> TerminalRenderDamageSnapshot {
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

    fn render_read(&mut self, force_full: bool) -> TerminalRenderRead {
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

impl CustomBackend {
    pub(super) fn new(
        size: TerminalSize,
        working_dir: Option<&str>,
        wakeup: Option<Sender<()>>,
        shell_integration: Option<&TabTitleShellIntegration>,
        config: Option<&TerminalRuntimeConfig>,
        startup: Option<&str>,
    ) -> anyhow::Result<Self> {
        let notifier = wakeup.map(|sender| {
            TerminalWakeupNotifier::new(move || {
                let _ = sender.try_send(());
            })
        });
        Self::new_with_wakeup_notifier(
            size,
            working_dir,
            notifier,
            shell_integration,
            config,
            startup,
        )
    }

    pub(super) fn new_with_wakeup_notifier(
        size: TerminalSize,
        working_dir: Option<&str>,
        notifier: Option<TerminalWakeupNotifier>,
        shell_integration: Option<&TabTitleShellIntegration>,
        config: Option<&TerminalRuntimeConfig>,
        startup: Option<&str>,
    ) -> anyhow::Result<Self> {
        let launch = startup.map(|command| TerminalLaunch::ShellCommand(command.to_owned()));
        Self::new_with_launch_and_wakeup_notifier(
            size,
            working_dir,
            notifier,
            shell_integration,
            config,
            launch.as_ref(),
            None,
            ChildEnvironment::Process,
        )
    }

    /// `restored_output` (a saved buffer) is replayed before the child starts,
    /// so it can't interleave with the new session's first output. The child
    /// inherits `source`, see [`ChildEnvironment`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_with_launch_and_wakeup_notifier(
        size: TerminalSize,
        working_dir: Option<&str>,
        notifier: Option<TerminalWakeupNotifier>,
        shell_integration: Option<&TabTitleShellIntegration>,
        config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
        restored_output: Option<&[u8]>,
        source: ChildEnvironment,
    ) -> anyhow::Result<Self> {
        if !engine::transport::available() {
            anyhow::bail!("the native terminal transport is unavailable on this host");
        }
        let config = config.cloned().unwrap_or_default();
        let mut terminal = Self::new_display_with_wakeup_notifier(size, Some(&config), notifier);
        if let Some(bytes) = restored_output {
            terminal.hydrate_output(bytes);
        }
        #[cfg(windows)]
        let interactive_shell = launch.is_none_or(|launch| {
            matches!(launch, TerminalLaunch::ShellCommand(command) if command.trim().is_empty())
        });
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut launch = resolve_terminal_launch_in(&config, launch, source)?;
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut environment = terminal_environment_overrides_in(shell_integration, &config, source);
        #[cfg(windows)]
        if interactive_shell {
            super::apply_windows_shell_integration(
                &mut launch,
                &mut environment,
                shell_integration,
                source.var_os("PROMPT").as_deref(),
            );
        }
        let working_directory =
            resolve_launch_working_directory(working_dir, config.working_dir_fallback);
        #[cfg(windows)]
        super::warn_cmd_unc_working_directory(&launch, working_directory.as_deref());
        let inherit_environment = matches!(source, ChildEnvironment::Process);
        let environment = match source {
            ChildEnvironment::Process => {
                let mut environment = environment.into_iter().collect::<Vec<_>>();
                // A stable order keeps the child environment block reproducible.
                environment.sort_unstable();
                environment
            }
            ChildEnvironment::Explicit(base) => explicit_child_environment(base, environment),
        };
        let spawn = SpawnConfig {
            program: launch.program,
            args: launch.args,
            working_directory,
            environment,
        };
        let shared = terminal.shared.clone();
        let exit = terminal.shared.clone();
        // The engine answers terminal queries through `feed`'s replies, which
        // lets a Windows pseudoconsole inherit the restored cursor position.
        terminal.transport = Some(Arc::new(Transport::spawn_answering_queries(
            spawn,
            pty_size(terminal.size()),
            inherit_environment,
            move |bytes| shared.feed(bytes, false, false),
            move || {
                let mut state = exit.state();
                if state.engine.stop_synchronized_update() {
                    let replies = state.take_engine_output();
                    state.append_replies(&replies);
                    state.generation = state.generation.wrapping_add(1);
                }
                state.engine.process_exited();
                state.queue(PendingEvent::Terminal(TerminalEvent::Exit));
                drop(state);
                // The exit clears program status and closes or marks the tab,
                // so hidden tabs must hear about it too.
                exit.notify_tab_chrome();
            },
        )?));
        let _ = terminal
            .shared
            .transport
            .set(Arc::downgrade(terminal.transport.as_ref().unwrap()));
        Ok(terminal)
    }

    pub(super) fn new_display(size: TerminalSize, config: Option<&TerminalRuntimeConfig>) -> Self {
        Self::new_display_with_wakeup_notifier(size, config, None)
    }

    pub(super) fn new_display_with_wakeup_notifier(
        size: TerminalSize,
        config: Option<&TerminalRuntimeConfig>,
        notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: lock::EngineMutex::new(State::new(
                    size,
                    &config.cloned().unwrap_or_default(),
                )),
                lock_waiters: lock::LockWaiters::default(),
                clipboard: Mutex::new(KittyClipboardHostState::new()),
                deferred_clipboard: Mutex::default(),
                notifier,
                wakeup_enabled: AtomicBool::new(true),
                wakeup_queued: AtomicBool::new(false),
                transport: OnceLock::new(),
                sync_signal: Arc::new(SyncSignal::default()),
                sync_watchdog_started: AtomicBool::new(false),
                input_dropped: AtomicBool::new(false),
            }),
            transport: None,
        }
    }

    pub(super) fn feed_output(&self, bytes: &[u8]) {
        let replies = self.shared.feed(bytes, false, self.transport.is_none());
        self.send_reply(replies);
    }

    pub(super) fn hydrate_output(&self, bytes: &[u8]) {
        self.shared.feed(bytes, true, false);
    }
    pub(super) fn child_pid(&self) -> Option<u32> {
        self.transport
            .as_ref()
            .map(|transport| transport.child_pid())
    }
    pub(super) fn terminate_and_wait(&self) {
        if let Some(transport) = &self.transport {
            transport.terminate_and_wait();
        }
    }
    pub(super) fn set_wakeup_enabled(&self, enabled: bool) {
        let old = self.shared.wakeup_enabled.swap(enabled, Ordering::AcqRel);
        if enabled && !old {
            self.shared.wakeup_queued.store(false, Ordering::Release);
            self.shared.notify();
        }
    }
    pub(super) fn write(&self, input: &[u8]) {
        if let Some(transport) = &self.transport {
            self.input_result(transport.write(input));
        }
    }
    pub(super) fn write_owned(&self, input: Vec<u8>) {
        if let Some(transport) = &self.transport {
            self.input_result(transport.write_owned(input));
        }
    }
    /// Motion reports may use only half the input backlog; when it is fuller
    /// they are dropped, and the next report carries the newer position.
    pub(super) fn write_mouse_motion(&self, input: Vec<u8>) {
        if let Some(transport) = &self.transport
            && let Err(error) = transport.write_motion_owned(input)
            && error.kind() != std::io::ErrorKind::WouldBlock
        {
            log::warn!("terminal input failed: {error}");
        }
    }
    fn input_result(&self, result: std::io::Result<()>) {
        let Err(error) = result else {
            return;
        };
        log::warn!("terminal input failed: {error}");
        if error.kind() == std::io::ErrorKind::WouldBlock
            && !self.shared.input_dropped.swap(true, Ordering::AcqRel)
        {
            self.shared.notify();
        }
    }
    #[inline]
    fn send_reply(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        self.send_nonempty_reply(bytes);
    }

    #[inline(never)]
    fn send_nonempty_reply(&self, bytes: Vec<u8>) {
        if let Some(transport) = &self.transport {
            if let Err(error) = transport.write_protocol_reply_owned(bytes) {
                log::warn!("terminal reply failed: {error}");
            }
        } else {
            self.shared.state().append_replies(&bytes);
        }
    }

    pub(super) fn resize(&mut self, size: TerminalSize) {
        let mut state = self.shared.state();
        let size = size.clamped();
        if state.size == size {
            return;
        }
        state.engine.resize(engine_size(size));
        state
            .engine
            .set_cell_pixels(size.cell_width, size.cell_height);
        if state.engine.take_history_activity() {
            state.defer_history_compaction(Instant::now());
        }
        let actual = state.engine.size();
        let size = TerminalSize {
            cols: actual.cols as u16,
            rows: actual.rows as u16,
            ..size
        };
        state.size = size;
        state.generation = state.generation.wrapping_add(1);
        state.force_full_damage = true;
        let replies = state.take_engine_output();
        self.shared.schedule_maintenance(&mut state);
        drop(state);
        self.send_reply(replies);
        if let Some(transport) = &self.transport
            && let Err(error) = transport.resize(pty_size(size))
        {
            log::warn!("terminal resize failed: {error}");
        }
        self.shared.notify();
    }
    pub(super) fn nudge_resize(&self) {
        if let Some(transport) = &self.transport
            && let Err(error) = transport.resize(pty_size(self.size()))
        {
            log::warn!("terminal resize notification failed: {error}");
        }
    }
    pub(super) fn size(&self) -> TerminalSize {
        self.shared.state().size
    }

    pub(super) fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        let mut state = self.shared.state();
        state.engine.graphics_snapshot()
    }
    pub(super) fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.kitty_graphics_snapshot().1
    }
    pub(super) fn kitty_graphics_revision(&self) -> u64 {
        self.shared.state().engine.poll_graphics_revision()
    }
    pub(super) fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        self.shared.state().engine.modes().clipboard_paste_events
    }
    pub(super) fn kitty_clipboard_paste_notification(
        &self,
        location: TerminalClipboardLocation,
        formats: &[String],
    ) -> Option<Vec<u8>> {
        let enabled = self.kitty_clipboard_paste_events_enabled();
        let mut clipboard = self
            .shared
            .clipboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        clipboard.set_paste_events_enabled(enabled);
        clipboard.paste_notification(location, formats)
    }
    pub(super) fn send_kitty_clipboard_paste_event(
        &self,
        location: TerminalClipboardLocation,
        formats: &[String],
    ) -> bool {
        let Some(notification) = self.kitty_clipboard_paste_notification(location, formats) else {
            return false;
        };
        // The notification stands for the user's paste: it queues behind the
        // keys typed before it instead of jumping ahead like a protocol
        // reply (audit L18).
        if self.transport.is_some() {
            self.write_owned(notification);
        } else {
            self.send_reply(notification);
        }
        true
    }

    pub(super) fn drain_events(
        &self,
        host: &mut impl TerminalReplyHost,
    ) -> (Vec<TerminalEvent>, bool) {
        let wakeup = self.shared.wakeup_queued.swap(false, Ordering::AcqRel);
        let (batch, has_more, program_status) = {
            let mut state = self.shared.state();
            let count = state.events.len().min(EVENT_BATCH);
            let batch: Vec<_> = (0..count).filter_map(|_| state.pop_event()).collect();
            // Released as each one is written; see `deferred`.
            state.ordered_in_flight += batch.iter().filter(|event| event.is_ordered()).count();
            let program_status = state.engine.take_program_status();
            (batch, !state.events.is_empty(), program_status)
        };
        let mut events = Vec::with_capacity(batch.len() + usize::from(wakeup));
        if wakeup {
            events.push(TerminalEvent::Wakeup);
        }
        if self.shared.input_dropped.swap(false, Ordering::AcqRel) {
            events.push(TerminalEvent::InputDropped);
        }
        // Deliver current state before Exit; it is not subject to queue overflow.
        if let Some(records) = program_status {
            events.push(TerminalEvent::ProgramStatus(records));
        }
        for event in batch {
            match event {
                PendingEvent::Terminal(event) => events.push(event),
                PendingEvent::ClipboardLoad(selection) => {
                    let target = if selection.contains('c') || selection.is_empty() {
                        TerminalClipboardTarget::Clipboard
                    } else {
                        TerminalClipboardTarget::Selection
                    };
                    self.ask_clipboard_host(
                        host,
                        TerminalClipboardRequest::Load(target),
                        deferred::Continuation::Load { selection },
                    );
                }
                PendingEvent::Kitty(packet) => {
                    let step = self
                        .shared
                        .clipboard
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .receive(packet);
                    match step {
                        KittyClipboardStep::Replies(replies) => {
                            self.write_ordered(deferred::Slot::Ready(replies));
                        }
                        KittyClipboardStep::Ask(request, pending) => self.ask_clipboard_host(
                            host,
                            request,
                            deferred::Continuation::Kitty(pending),
                        ),
                    }
                }
                PendingEvent::Reply(reply) => {
                    self.write_ordered(deferred::Slot::Ready(vec![reply]));
                }
                PendingEvent::KittyOverflow => {
                    self.shared
                        .clipboard
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .reset_preserving_paste_mode();
                    self.cancel_clipboard_requests();
                }
                PendingEvent::KittyControl(control) => {
                    let mut clipboard = self
                        .shared
                        .clipboard
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match control {
                        KittyClipboardControl::Set(enabled) => {
                            clipboard.set_paste_events_enabled(enabled);
                        }
                        KittyClipboardControl::Reset => {
                            clipboard.reset();
                            // A reset (also RIS) may start another program:
                            // a late answer must not reach it.
                            drop(clipboard);
                            self.cancel_clipboard_requests();
                        }
                        KittyClipboardControl::Query => {}
                    }
                }
            }
        }
        let replies = {
            let mut state = self.shared.state();
            std::mem::take(&mut state.replies)
        };
        if !replies.is_empty() {
            host.protocol_reply(&replies);
        }
        (events, has_more)
    }
    pub(super) fn has_pending_events(&self) -> bool {
        let state = self.shared.state();
        self.shared.wakeup_queued.load(Ordering::Acquire)
            || self.shared.input_dropped.load(Ordering::Acquire)
            || !state.events.is_empty()
            || !state.replies.is_empty()
            || state.engine.has_program_status_changes()
    }
    pub(super) fn set_query_colors(&mut self, colors: TerminalQueryColors) {
        let mut state = self.shared.state();
        state.query_colors = colors;
        state.engine.set_query_colors(colors);
        state.palette_epoch = state.palette_epoch.wrapping_add(1);
        state.force_full_damage = true;
    }
    pub(super) fn palette(&self) -> TerminalPalette {
        self.shared.state().palette()
    }
    pub(super) fn snapshot(&self) -> TermyFrame {
        let state = self.shared.state();
        let palette = state.palette();
        let mut cells =
            Vec::with_capacity(usize::from(state.size.cols) * usize::from(state.size.rows));
        let mut legacy = LegacyCells::new(&palette, state.query_colors);
        let mut scratch = Vec::new();
        for row in 0..usize::from(state.size.rows) {
            state.engine.with_viewport_row_blanks(
                row,
                &mut scratch,
                |line, wrapped, blanks, background| {
                    legacy.extend_row(&mut cells, line, wrapped, blanks, background);
                },
            );
        }
        TermyFrame {
            cols: state.size.cols,
            rows: state.size.rows,
            cells,
            cursor: state.cursor(),
            display_offset: state.engine.display_offset(),
            history_size: state.engine.history_size(),
        }
    }
    pub(super) fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        let mut state = self.shared.state();
        let mut update = state.take_damage(force_full);
        expand_legacy_scroll_damage(&mut update, usize::from(state.size.cols));
        let palette = state.palette();
        let full_rows: Vec<_>;
        let spans = match &update.damage {
            TerminalDamageSnapshot::Full => {
                full_rows = (0..usize::from(state.size.rows))
                    .map(|row| TerminalDirtySpan {
                        row,
                        left_col: 0,
                        right_col: usize::from(state.size.cols) - 1,
                    })
                    .collect();
                &full_rows
            }
            TerminalDamageSnapshot::Partial(spans) => spans,
        };
        let mut cells = crate::frame::frame_cells(
            spans
                .iter()
                .map(|span| span.right_col + 1 - span.left_col.min(span.right_col + 1))
                .sum(),
        );
        let mut legacy = LegacyCells::new(&palette, state.query_colors);
        let mut scratch = Vec::new();
        for &span in spans {
            state.engine.with_viewport_row_blanks(
                span.row,
                &mut scratch,
                |line, wrapped, blanks, background| {
                    let right = span.right_col.min(line.len() - 1);
                    if span.left_col > right {
                        return;
                    }
                    // Only the row's last cell carries its wrap flag.
                    let wraps = wrapped && right + 1 == line.len();
                    legacy.extend_row(
                        &mut cells,
                        &line[span.left_col..=right],
                        wraps,
                        blanks.saturating_sub(span.left_col),
                        background,
                    );
                },
            );
        }
        TermyFrameUpdate {
            cols: state.size.cols,
            rows: state.size.rows,
            cells,
            cursor: state.cursor(),
            display_offset: state.engine.display_offset(),
            history_size: state.engine.history_size(),
            damage: update.damage,
        }
    }
    pub(super) fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        self.shared.state().take_damage(false)
    }
    pub(super) fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        let mut state = self.shared.state();
        let mut update = state.take_damage(false);
        expand_legacy_scroll_damage(&mut update, usize::from(state.size.cols));
        update.damage
    }
    pub(super) fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        self.shared.state().render_read(force_full)
    }
    pub(super) fn render_read_with_screen(&self, force_full: bool) -> (TerminalRenderRead, bool) {
        let mut state = self.shared.state();
        let alternate = state.engine.alternate_screen();
        (state.render_read(force_full), alternate)
    }
    pub(super) fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        self.search_with_options(query, TermySearchOptions::default())
    }
    pub(super) fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        self.search_shared_with_options(query, options)
            .into_iter()
            .map(Into::into)
            .collect()
    }
    pub(super) fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        self.search_shared_with_options(query, TermySearchOptions::default())
    }
    pub(super) fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        if query.is_empty() {
            return Vec::new();
        }
        // Copy the text under the engine lock, then compile the pattern and
        // match without it, so a long search doesn't stall PTY output.
        let state = self.shared.state();
        let history = state.engine.history_size() as i32;
        let mut scratch = Vec::new();
        let lines = (-history..i32::from(state.size.rows))
            .filter_map(|line| {
                state.engine.with_line(line, &mut scratch, |cells| {
                    let mut text = String::with_capacity(cells.len());
                    let mut mapping = SearchLineMapping::default();
                    for (col, cell) in cells.iter().enumerate() {
                        if cell.flags & engine::Cell::WIDE_SPACER != 0 {
                            continue;
                        }
                        let start = text.len();
                        append_search_cell(&mut text, cell, cells.combining(col));
                        mapping.record_cell(
                            &text,
                            start,
                            col,
                            1 + usize::from(cell.flags & engine::Cell::WIDE != 0),
                        );
                    }
                    text.truncate(text.trim_end().len());
                    ((line + history) as usize, text, mapping)
                })
            })
            .collect::<Vec<_>>();
        drop(state);
        search_lines_shared(lines, query, options)
    }
    pub(super) fn hyperlink_at(&self, row: usize, col: usize) -> Option<DetectedLink> {
        crate::links::hyperlink_at_viewport_cell(&self.shared.state().engine, row, col)
    }

    pub(super) fn link_at(&self, row: usize, col: usize) -> Option<DetectedViewportLink> {
        // Only the grid lookup needs the engine lock; checking a path on the
        // filesystem can be slow, so it runs after the lock is released.
        let candidate = {
            let state = self.shared.state();
            crate::links::link_candidate_at_viewport_cell(&state.engine, row, col)
        };
        candidate?.resolve()
    }

    pub(super) fn scroll_display(&self, delta: i32) -> bool {
        let mut state = self.shared.state();
        let changed = state.engine.scroll_display(delta);
        if changed {
            state.generation = state.generation.wrapping_add(1);
        }
        drop(state);
        if changed {
            self.shared.notify();
        }
        changed
    }
    pub(super) fn scroll_to_bottom(&self) -> bool {
        self.scroll_display(i32::MIN)
    }
    pub(super) fn clear_scrollback(&self) -> bool {
        let mut state = self.shared.state();
        let changed = state.engine.history_size() > 0;
        state.engine.clear_scrollback();
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        if changed {
            self.shared.notify();
        }
        changed
    }
    pub(super) fn scroll_state(&self) -> (usize, usize) {
        let state = self.shared.state();
        (state.engine.display_offset(), state.engine.history_size())
    }
    pub(super) fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.shared.state().cursor()
    }
    pub(super) fn cursor_position(&self) -> (usize, usize) {
        let cursor = self.shared.state().engine.cursor();
        (cursor.col, cursor.row)
    }
    pub(super) fn set_term_options(&self, options: TerminalOptions) {
        let mut state = self.shared.state();
        let current = state.engine.options();
        state.engine.set_options(engine::Options {
            scrollback_history: options
                .scrollback_history
                .min(MAX_TERMINAL_SCROLLBACK_HISTORY),
            ..current
        });
        state
            .engine
            .set_default_cursor_shape(cursor_shape(options.default_cursor_style));
        state.default_cursor_style = options.default_cursor_style;
        state.generation = state.generation.wrapping_add(1);
        state.force_full_damage = true;
    }
    pub(super) fn set_scrollback_history(&self, history: usize) {
        let style = self.shared.state().default_cursor_style;
        self.set_term_options(TerminalOptions {
            scrollback_history: history,
            default_cursor_style: style,
        });
    }
    pub(super) fn bracketed_paste_mode(&self) -> bool {
        self.shared.state().engine.modes().bracketed_paste
    }
    pub(super) fn alternate_screen_mode(&self) -> bool {
        self.shared.state().engine.alternate_screen()
    }
    pub(super) fn keyboard_mode(&self) -> TerminalKeyboardMode {
        let modes = self.shared.state().engine.modes();
        let bits = modes.kitty_keyboard;
        TerminalKeyboardMode::from_flags(
            modes.application_cursor,
            bits & 1 != 0,
            bits & 2 != 0,
            bits & 4 != 0,
            bits & 8 != 0,
            bits & 16 != 0,
        )
        .with_application_keypad(modes.application_keypad)
    }
    pub(super) fn mouse_mode(&self) -> TerminalMouseMode {
        let modes = self.shared.state().engine.modes();
        TerminalMouseMode {
            enabled: modes.mouse_tracking != engine::MouseTracking::None,
            // X10 (mode 9) reports presses only: enabled with no other flag.
            report_click: modes.mouse_tracking == engine::MouseTracking::Click,
            report_drag: modes.mouse_tracking == engine::MouseTracking::Drag,
            report_motion: modes.mouse_tracking == engine::MouseTracking::Motion,
            sgr_encoding: modes.mouse_encoding == engine::MouseEncoding::Sgr,
            utf8_encoding: modes.mouse_encoding == engine::MouseEncoding::Utf8,
        }
    }
}

fn engine_size(size: TerminalSize) -> engine::Size {
    engine::Size {
        cols: usize::from(size.cols),
        rows: usize::from(size.rows),
    }
}
fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}
fn cursor_shape(style: TerminalCursorStyle) -> engine::CursorShape {
    match style {
        TerminalCursorStyle::Block => engine::CursorShape::Block,
        TerminalCursorStyle::Line => engine::CursorShape::Beam,
    }
}
fn rgb(color: engine::Color) -> Option<TerminalColor> {
    color.as_rgb().map(|(r, g, b)| TerminalColor { r, g, b })
}
fn render_color(color: engine::Color, foreground: bool) -> TerminalRenderColor {
    if let Some(rgb) = rgb(color) {
        TerminalRenderColor::Rgb(rgb)
    } else if let Some(index) = color.as_indexed() {
        TerminalRenderColor::Indexed(index)
    } else if foreground {
        TerminalRenderColor::DefaultForeground
    } else {
        TerminalRenderColor::DefaultBackground
    }
}
/// The cell of `line` at `col`, with its metadata.
fn render_cell(line: &engine::Line<'_>, col: usize, wrapped: bool) -> TerminalRenderCell {
    let cell = &line[col];
    let style = cell.style;
    let attr = style.attributes;
    let underline_color = line.underline_color(col);
    TerminalRenderCell {
        text: TerminalRenderText::from_cell_suffix(cell.character, Some(line.combining(col))),
        foreground: render_color(style.foreground, true),
        background: render_color(style.background, false),
        underline_color: (underline_color != engine::Color::DEFAULT)
            .then(|| render_color(underline_color, true)),
        bold: attr & engine::Style::BOLD != 0,
        dim: attr & engine::Style::DIM != 0,
        italic: attr & engine::Style::ITALIC != 0,
        inverse: attr & engine::Style::INVERSE != 0,
        hidden: attr & engine::Style::HIDDEN != 0,
        strikethrough: attr & engine::Style::STRIKE != 0,
        underline_style: match style.underline {
            engine::UnderlineStyle::None => TerminalUnderlineStyle::None,
            engine::UnderlineStyle::Single => TerminalUnderlineStyle::Single,
            engine::UnderlineStyle::Double => TerminalUnderlineStyle::Double,
            engine::UnderlineStyle::Curly => TerminalUnderlineStyle::Curly,
            engine::UnderlineStyle::Dotted => TerminalUnderlineStyle::Dotted,
            engine::UnderlineStyle::Dashed => TerminalUnderlineStyle::Dashed,
        },
        hyperlink: line.hyperlink(col).is_some(),
        wide_character_spacer: cell.flags & engine::Cell::WIDE_SPACER != 0,
        leading_wide_character_spacer: cell.flags & engine::Cell::LEADING_WIDE_SPACER != 0,
        line_wrapped: wrapped,
    }
}
fn resolve_color(
    color: TerminalRenderColor,
    palette: &TerminalPalette,
    fallback: TerminalQueryColors,
) -> TermyColor {
    let color = match color {
        TerminalRenderColor::Rgb(rgb) => rgb,
        TerminalRenderColor::Indexed(index) | TerminalRenderColor::DimIndexed(index) => {
            palette.indexed[index as usize].unwrap_or_else(|| fallback.indexed_color(index))
        }
        TerminalRenderColor::DefaultBackground => palette.background.unwrap_or(fallback.background),
        TerminalRenderColor::Cursor => palette
            .cursor
            .or(fallback.cursor)
            .unwrap_or(fallback.foreground),
        _ => palette.foreground.unwrap_or(fallback.foreground),
    };
    TermyColor {
        r: color.r,
        g: color.g,
        b: color.b,
        a: 255,
    }
}
fn append_search_cell(text: &mut String, cell: &engine::Cell, combining: &str) {
    if cell.flags & engine::Cell::WIDE_SPACER != 0 {
        return;
    }
    if cell.flags & engine::Cell::LEADING_WIDE_SPACER != 0
        || cell.style.attributes & engine::Style::HIDDEN != 0
        || cell.character.is_control()
    {
        text.push(' ');
    } else {
        text.push(cell.character);
        text.push_str(combining);
    }
}
// Legacy frame consumers replay only cell patches, so include the rows moved
// by scroll operations rather than silently dropping that part of the update.
fn expand_legacy_scroll_damage(update: &mut TerminalRenderDamageSnapshot, cols: usize) {
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
fn add_spans(
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

fn normalize_spans(spans: &mut Vec<TerminalDirtySpan>) {
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

#[cfg(test)]
mod tests;
