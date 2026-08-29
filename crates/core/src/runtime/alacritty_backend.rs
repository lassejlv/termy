use super::*;
use crate::kitty_graphics_placeholders_from_alacritty_grid;

/// Complete state for the retained Alacritty implementation.
///
/// Keeping engine-owned state in one private value gives the public
/// terminal facade a single replacement boundary for the temporary
/// dual-backend phase.
pub(super) struct AlacrittyBackend {
    term: Arc<FairMutex<Term<JsonEventListener>>>,
    listener: JsonEventListener,
    parser: FairMutex<ansi::Processor>,
    kitty_clipboard_interceptor: FairMutex<KittyClipboardInterceptor>,
    kitty_clipboard: FairMutex<KittyClipboardHostState>,
    kitty_graphics_interceptor: FairMutex<KittyGraphicsInterceptor>,
    kitty_graphics_cursor_tracker: FairMutex<KittyGraphicsCursorTracker>,
    kitty_graphics: Arc<FairMutex<KittyGraphicsState>>,
    kitty_graphics_revision: Arc<AtomicU64>,
    render_generation: Arc<AtomicU64>,
    palette_revision: Arc<AtomicU64>,
    palette_snapshot: Arc<FairMutex<crate::TerminalPalette>>,
    resize_anchor_state: Arc<crate::resize_anchor::ResizeAnchorState>,
    graphics_size: Arc<FairMutex<TerminalSize>>,
    pty_tx: Option<EventLoopSender>,
    pending_protocol_replies: FairMutex<Vec<u8>>,
    events_rx: Receiver<RuntimeEvent>,
    size: TerminalSize,
    query_colors: TerminalQueryColors,
    default_cursor_style: TerminalCursorStyle,
    child_pid: Option<u32>,
}

impl AlacrittyBackend {
    /// Create a new terminal with the given size.
    pub fn new(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        event_wakeup_tx: Option<Sender<()>>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        let wakeup_notifier = event_wakeup_tx.map(|event_wakeup_tx| {
            TerminalWakeupNotifier::new(move || {
                let _ = event_wakeup_tx.try_send(());
            })
        });
        Self::new_with_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
    }

    /// Create a terminal whose wakeups are routed through a host callback.
    ///
    /// Multi-terminal hosts can capture a stable terminal identifier in the
    /// callback and drain only the terminal that produced the wakeup.
    pub fn new_with_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        let launch =
            startup_command.map(|command| TerminalLaunch::ShellCommand(command.to_string()));
        Self::new_with_launch_and_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            launch.as_ref(),
        )
    }

    /// Create a terminal whose child is selected with a typed launch contract.
    pub fn new_with_launch_and_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
    ) -> anyhow::Result<Self> {
        let size = size.clamped();
        let (events_tx, events_rx) = unbounded();
        let runtime_config = runtime_config.cloned().unwrap_or_default();
        let shell = launch_to_shell(resolve_terminal_launch(&runtime_config, launch)?);

        let working_directory = resolve_launch_working_directory(
            configured_working_dir,
            runtime_config.working_dir_fallback,
        );

        let pty_options = PtyOptions {
            shell: Some(shell),
            working_directory,
            env: terminal_environment_overrides(tab_title_shell_integration, &runtime_config),
            drain_on_exit: true,
            #[cfg(target_os = "windows")]
            escape_args: true,
        };

        let term_config = backend::term_config(runtime_config.term_options());

        let listener = JsonEventListener::new_with_wakeup_notifier(events_tx, wakeup_notifier);
        let term = Term::new(term_config, &size, listener.clone());
        let palette_snapshot = backend::palette(&term, 0);
        let term = Arc::new(FairMutex::new(term));
        let kitty_graphics = Arc::new(FairMutex::new(KittyGraphicsState::default()));
        let kitty_graphics_revision = Arc::new(AtomicU64::new(0));
        let render_generation = Arc::new(AtomicU64::new(0));
        let palette_revision = Arc::new(AtomicU64::new(0));
        let palette_snapshot = Arc::new(FairMutex::new(palette_snapshot));
        let resize_anchor_state = Arc::new(crate::resize_anchor::ResizeAnchorState::default());
        let graphics_size = Arc::new(FairMutex::new(size));

        let window_id = 0;
        let pty = tty::new(&pty_options, size.into(), window_id)?;
        #[cfg(unix)]
        let child_pid = Some(pty.child().id());
        #[cfg(not(unix))]
        let child_pid = pty_child_pid(&pty);

        let event_loop = NativeEventLoop::new(
            NativeRenderState {
                terminal: term.clone(),
                generation: render_generation.clone(),
                palette_revision: palette_revision.clone(),
                palette_snapshot: palette_snapshot.clone(),
            },
            kitty_graphics.clone(),
            kitty_graphics_revision.clone(),
            resize_anchor_state.clone(),
            graphics_size.clone(),
            listener.clone(),
            pty,
            pty_options.drain_on_exit,
        )?;
        let pty_tx = event_loop.channel();
        event_loop.spawn();
        log::info!(
            "terminal runtime started; kitty graphics logging active (RUST_LOG=termy_core=debug for per-command detail)"
        );

        Ok(Self {
            term,
            listener,
            parser: FairMutex::new(ansi::Processor::new()),
            kitty_clipboard_interceptor: FairMutex::new(KittyClipboardInterceptor::default()),
            kitty_clipboard: FairMutex::new(KittyClipboardHostState::new()),
            kitty_graphics_interceptor: FairMutex::new(KittyGraphicsInterceptor::default()),
            kitty_graphics_cursor_tracker: FairMutex::new(KittyGraphicsCursorTracker::default()),
            kitty_graphics,
            kitty_graphics_revision,
            render_generation,
            palette_revision,
            palette_snapshot,
            resize_anchor_state,
            graphics_size,
            pty_tx: Some(pty_tx),
            pending_protocol_replies: FairMutex::new(Vec::new()),
            events_rx,
            size,
            query_colors: runtime_config.query_colors,
            default_cursor_style: runtime_config.default_cursor_style,
            child_pid,
        })
    }

    /// Create a display-only terminal: a grid + parser with no PTY/shell. Its
    /// content is supplied with [`Terminal::feed_output`] (e.g. tmux `%output`).
    /// All rendering, sizing, and event draining work as for a normal terminal;
    /// input (`write`) is a no-op since there is no child process.
    #[cfg(test)]
    pub fn new_display(size: TerminalSize, runtime_config: Option<&TerminalRuntimeConfig>) -> Self {
        Self::new_display_with_wakeup_notifier(size, runtime_config, None)
    }

    pub fn new_display_with_wakeup_notifier(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        let size = size.clamped();
        let (events_tx, events_rx) = unbounded();
        let runtime_config = runtime_config.cloned().unwrap_or_default();
        let term_config = backend::term_config(runtime_config.term_options());
        let listener = JsonEventListener::new_with_wakeup_notifier(events_tx, wakeup_notifier);
        let term = Term::new(term_config, &size, listener.clone());
        let palette_snapshot = backend::palette(&term, 0);
        let term = Arc::new(FairMutex::new(term));
        let kitty_graphics = Arc::new(FairMutex::new(KittyGraphicsState::default()));
        let kitty_graphics_revision = Arc::new(AtomicU64::new(0));
        let render_generation = Arc::new(AtomicU64::new(0));
        let palette_revision = Arc::new(AtomicU64::new(0));
        let palette_snapshot = Arc::new(FairMutex::new(palette_snapshot));
        let resize_anchor_state = Arc::new(crate::resize_anchor::ResizeAnchorState::default());

        Self {
            term,
            listener,
            parser: FairMutex::new(ansi::Processor::new()),
            kitty_clipboard_interceptor: FairMutex::new(KittyClipboardInterceptor::default()),
            kitty_clipboard: FairMutex::new(KittyClipboardHostState::new()),
            kitty_graphics_interceptor: FairMutex::new(KittyGraphicsInterceptor::default()),
            kitty_graphics_cursor_tracker: FairMutex::new(KittyGraphicsCursorTracker::default()),
            kitty_graphics,
            kitty_graphics_revision,
            render_generation,
            palette_revision,
            palette_snapshot,
            resize_anchor_state,
            graphics_size: Arc::new(FairMutex::new(size)),
            pty_tx: None,
            pending_protocol_replies: FairMutex::new(Vec::new()),
            events_rx,
            size,
            query_colors: runtime_config.query_colors,
            default_cursor_style: runtime_config.default_cursor_style,
            child_pid: None,
        }
    }

    /// Advance the grid with output bytes without involving a PTY. Used by
    /// display-only terminals (tmux `%output`); damage is recorded so the next
    /// render picks up the change.
    pub fn feed_output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.feed_output_to_parser(bytes);
        if self.pty_tx.is_none() {
            self.listener.send_event(AlacEvent::Wakeup);
        }
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    pub fn set_wakeup_enabled(&self, enabled: bool) {
        self.listener.set_wakeup_enabled(enabled);
    }

    /// Write bytes to the PTY (user input). No-op for display-only terminals.
    pub fn write(&self, input: &[u8]) {
        if self.pty_tx.is_some() {
            self.write_owned(input.to_vec());
        }
    }

    /// Write owned bytes to the PTY without copying them into the event-loop
    /// channel. Prefer this when an encoder already produced a `Vec<u8>`.
    /// No-op for display-only terminals.
    pub fn write_owned(&self, input: Vec<u8>) {
        if let Some(pty_tx) = &self.pty_tx {
            let _ = pty_tx.send(EventLoopMsg::Input(input.into()));
        }
    }

    /// Rehydrate saved terminal output into the in-memory grid without sending input to the PTY.
    pub fn hydrate_output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        self.listener.set_replay_suppressed(true);
        self.feed_output_to_parser(bytes);
        self.listener.set_replay_suppressed(false);
    }

    fn feed_output_to_parser(&self, bytes: &[u8]) {
        let (filtered, kitty_clipboard_inputs) =
            self.kitty_clipboard_interceptor.lock().process(bytes);
        for input in kitty_clipboard_inputs {
            self.listener.send_kitty_clipboard_input(input);
        }
        let mut interceptor = self.kitty_graphics_interceptor.lock();
        let mut cursor_tracker = self.kitty_graphics_cursor_tracker.lock();
        let mut parser = self.parser.lock();
        let mut term = self.term.lock();
        let mut graphics_changed = false;
        let mut term_mutated = false;
        for item in interceptor.process(&filtered) {
            match item {
                KittyGraphicsItem::Text(text) => {
                    term_mutated = true;
                    let track_scrolls = self.kitty_graphics.lock().has_placements();
                    let effects = advance_terminal_text(
                        &mut cursor_tracker,
                        &mut parser,
                        &mut term,
                        &text,
                        track_scrolls,
                        Some(&self.resize_anchor_state),
                    );
                    if !effects.is_empty() {
                        graphics_changed |= effects.apply_to(&mut self.kitty_graphics.lock());
                    }
                }
                KittyGraphicsItem::Command(command) => {
                    let cursor = term.grid().cursor.point;
                    let full_screen_scroll_region =
                        cursor_tracker.region_covers_full_screen(term.grid().screen_lines());
                    let screen = KittyGraphicsScreen::from_alternate_screen(
                        term.mode().contains(TermMode::ALT_SCREEN),
                    );
                    let result = self.kitty_graphics.lock().apply_on_screen(
                        command,
                        cursor.column.0,
                        cursor.line.0.max(0) as usize,
                        term.grid().history_size(),
                        self.size,
                        screen,
                    );
                    graphics_changed |= result.changed;
                    if result.cursor_advance_screen == Some(screen)
                        && let Some((cols, rows)) = result.cursor_advance
                    {
                        let untracked_scroll = move_graphics_cursor_and_record(
                            &mut *term,
                            cols,
                            rows,
                            full_screen_scroll_region,
                            &self.render_generation,
                            &self.palette_revision,
                            &self.palette_snapshot,
                        );
                        if untracked_scroll > 0 {
                            graphics_changed |= self
                                .kitty_graphics
                                .lock()
                                .scroll_up_without_history_on_screen(untracked_scroll, screen);
                        }
                    }
                }
            }
        }
        if graphics_changed {
            self.kitty_graphics_revision.fetch_add(1, Ordering::Relaxed);
        }
        if term_mutated {
            self.record_render_mutation(&term);
        }
    }

    /// Write a string to the PTY
    #[allow(dead_code)]
    pub fn write_str(&self, input: &str) {
        self.write(input.as_bytes());
    }

    /// Resize the terminal. Identical sizes are ignored; use [`Self::nudge_resize`]
    /// when the child needs a fresh `SIGWINCH` without a dimension change.
    pub fn resize(&mut self, new_size: TerminalSize) {
        let new_size = new_size.clamped();
        if self.size == new_size {
            return;
        }
        self.size = new_size;
        *self.graphics_size.lock() = new_size;
        if let Some(pty_tx) = &self.pty_tx {
            let _ = pty_tx.send(EventLoopMsg::Resize(new_size.into()));
        }
        let mut term = self.term.lock();
        term.resize(new_size);
        self.kitty_graphics_cursor_tracker
            .lock()
            .reset_scroll_region();
        // Keep content bottom-anchored like Ghostty/Kitty: reflow can strand
        // the prompt mid-screen above blank rows while the start of the
        // output sits in scrollback — pull it back in.
        if self.resize_anchor_state.allows_restore() {
            crate::resize_anchor::restore_bottom_anchor(&mut term, new_size);
        }
        self.record_render_mutation(&term);
    }

    /// Re-send the current size to the PTY without touching the term grid.
    /// This delivers SIGWINCH to the child process, nudging TUI applications
    /// (e.g. lazygit) to refresh their display after an alternate-screen
    /// transition even though the actual dimensions have not changed.
    pub fn nudge_resize(&self) {
        if let Some(pty_tx) = &self.pty_tx {
            let _ = pty_tx.send(EventLoopMsg::NudgeResize(self.size.into()));
        }
    }

    /// Get the current terminal size
    pub fn size(&self) -> TerminalSize {
        self.size
    }

    /// Snapshot visible Kitty graphics placements for a renderer.
    pub fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.kitty_graphics_snapshot().1
    }

    /// Monotonic revision for Kitty image or placement state.
    pub fn kitty_graphics_revision(&self) -> u64 {
        self.kitty_graphics_revision.load(Ordering::Relaxed)
    }

    /// Snapshot the current Kitty revision and visible placements together.
    pub fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        let term = self.term.lock();
        let grid = term.grid();
        let screen =
            KittyGraphicsScreen::from_alternate_screen(term.mode().contains(TermMode::ALT_SCREEN));
        let placeholders = kitty_graphics_placeholders_from_alacritty_grid(grid);
        let placements = self
            .kitty_graphics
            .lock()
            .render_placements_on_screen_with_placeholders(
                grid.history_size(),
                grid.display_offset(),
                grid.screen_lines(),
                grid.columns(),
                screen,
                &placeholders,
            );
        (
            self.kitty_graphics_revision.load(Ordering::Relaxed),
            placements,
        )
    }

    pub fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        self.kitty_clipboard.lock().paste_events_enabled()
    }

    pub fn send_kitty_clipboard_paste_event(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> bool {
        let Some(notification) = self
            .kitty_clipboard
            .lock()
            .paste_notification(location, available_formats)
        else {
            return false;
        };
        if self.pty_tx.is_some() {
            self.write_owned(notification);
        } else {
            self.pending_protocol_replies.lock().extend(notification);
        }
        true
    }

    /// Drain pending Alacritty events, writing reply bytes back to the PTY when required.
    /// Returns the collected events and whether more events remain (batch limit hit).
    pub fn drain_events(&self, host: &mut impl TerminalReplyHost) -> (Vec<TerminalEvent>, bool) {
        let pending_protocol_replies = {
            let mut pending = self.pending_protocol_replies.lock();
            std::mem::take(&mut *pending)
        };
        if !pending_protocol_replies.is_empty() {
            host.protocol_reply(&pending_protocol_replies);
        }

        // Reset before probing the queue. A previous drain can consume a Wakeup
        // queued concurrently while leaving the coalescing flag set. Another
        // PTY update can then be folded into that flag without queueing a new
        // event, so an empty queue still requires one final redraw.
        let wakeup_was_queued = self.listener.reset_wakeup_queued();

        // Consume the first item before allocating drain state so a coalesced
        // or otherwise spurious host drain stays allocation-free without an
        // `is_empty`/`try_recv` race.
        let Ok(first_event) = self.events_rx.try_recv() else {
            let events = if wakeup_was_queued {
                vec![TerminalEvent::Wakeup]
            } else {
                Vec::new()
            };
            return (events, false);
        };

        drain_runtime_events(
            first_event,
            &self.events_rx,
            self.size,
            &self.term,
            self.query_colors,
            &self.kitty_clipboard,
            host,
            |response| {
                let has_transport = self.pty_tx.is_some();
                self.write(response);
                has_transport
            },
        )
    }

    pub fn set_query_colors(&mut self, query_colors: TerminalQueryColors) {
        self.query_colors = query_colors;
    }

    fn record_render_mutation<T: EventListener>(&self, term: &Term<T>) {
        record_render_mutation(
            term,
            &self.render_generation,
            &self.palette_revision,
            &self.palette_snapshot,
        );
    }

    fn refresh_palette_revision<T: EventListener>(&self, term: &Term<T>) {
        refresh_palette_revision(term, &self.palette_revision, &self.palette_snapshot);
    }

    pub fn palette(&self) -> crate::TerminalPalette {
        let term = self.term.lock();
        self.refresh_palette_revision(&term);
        self.palette_snapshot.lock().clone()
    }

    /// Capture a renderer-neutral snapshot of the visible terminal frame.
    pub fn snapshot(&self) -> TermyFrame {
        self.with_term(|term| snapshot_from_term(term, self.size, self.query_colors))
    }

    /// Capture a damage-scoped visible-frame update for incremental renderers.
    pub fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        self.with_term_mut(|term| {
            let damage = if force_full {
                term.reset_damage();
                TerminalDamageSnapshot::Full
            } else {
                backend::take_damage_snapshot(term)
            };
            snapshot_update_from_term(term, self.size, self.query_colors, damage)
        })
    }

    /// Consume renderer-neutral damage with a generation for coherent partial reads.
    pub fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        let mut term = self.term.lock();
        self.refresh_palette_revision(&term);
        let damage = backend::take_damage_snapshot(&mut term);
        TerminalRenderDamageSnapshot {
            damage,
            scrolls: Vec::new(),
            generation: self.render_generation.load(Ordering::Relaxed),
            palette_revision: self.palette_revision.load(Ordering::Relaxed),
        }
    }

    /// Capture a coherent rich viewport read without exposing engine types.
    pub fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        let mut term = self.term.lock();
        self.refresh_palette_revision(&term);
        let damage = if force_full {
            term.reset_damage();
            TerminalDamageSnapshot::Full
        } else {
            backend::take_damage_snapshot(&mut term)
        };
        let generation = self.render_generation.load(Ordering::Relaxed);
        let cells = backend::visible_render_cells(&term);
        let (display_offset, history_size) = backend::scroll_state(&term);
        TerminalRenderRead {
            metadata: TerminalViewportMetadata {
                cols: self.size.cols,
                rows: self.size.rows,
                cursor: backend::cursor_state(&term),
                display_offset,
                history_size,
                palette_revision: self.palette_revision.load(Ordering::Relaxed),
                generation,
            },
            palette: self.palette_snapshot.lock().clone(),
            cells,
            update: TerminalRenderDamageSnapshot {
                damage,
                scrolls: Vec::new(),
                generation,
                palette_revision: self.palette_revision.load(Ordering::Relaxed),
            },
        }
    }

    /// Visit visible rich cells and return coherent viewport metadata.
    pub fn visit_viewport_cells(
        &self,
        mut visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        let (metadata, batch) = {
            let term = self.term.lock();
            self.refresh_palette_revision(&term);
            let batch = backend::viewport_cells(&term);
            let (_, history_size) = backend::scroll_state(&term);
            let metadata = TerminalViewportMetadata {
                cols: self.size.cols,
                rows: self.size.rows,
                cursor: backend::cursor_state(&term),
                display_offset: batch.display_offset,
                history_size,
                palette_revision: self.palette_revision.load(Ordering::Relaxed),
                generation: self.render_generation.load(Ordering::Relaxed),
            };
            (metadata, batch)
        };
        let cols = usize::from(metadata.cols);
        for (index, cell) in batch.cells.iter().enumerate() {
            let row = index / cols;
            let line = row as i32 - batch.display_offset as i32;
            visitor(batch.display_offset, line, index % cols, cell);
        }
        metadata
    }

    pub fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        mut visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        let Some(cells) = ({
            let term = self.term.lock();
            capture_viewport_ranges_at_generation(&term, &self.render_generation, generation, spans)
        }) else {
            return false;
        };
        for cell in &cells {
            visitor(
                cell.row,
                cell.display_offset,
                cell.line,
                cell.col,
                &cell.cell,
            );
        }
        true
    }

    pub fn line_bounds(&self) -> (i32, i32) {
        let term = self.term.lock();
        backend::line_bounds(&term)
    }

    /// Visit a requested inclusive buffer-line range from one coherent engine state.
    ///
    /// The callback runs while the backend is locked and must not call back into
    /// this terminal. Streaming cells under that lock keeps full-scrollback
    /// persistence bounded instead of cloning every rich cell first.
    pub fn visit_line_cells(
        &self,
        requested_first: i32,
        requested_last: i32,
        visitor: impl FnMut((i32, i32, usize), i32, usize, &crate::TerminalRenderCell),
    ) -> (i32, i32, usize) {
        let term = self.term.lock();
        backend::visit_line_cells(&term, requested_first, requested_last, visitor)
    }

    /// Search the full terminal buffer and return match ranges in scrollback-relative rows.
    pub fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        self.search_with_options(query, TermySearchOptions::default())
    }

    /// Search the full terminal buffer with explicit matching options.
    pub fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        self.with_term(|term| search_term_buffer(term, query, options))
    }

    /// Search the full terminal buffer while sharing line storage between
    /// matches on the same row.
    pub fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        self.search_shared_with_options(query, TermySearchOptions::default())
    }

    /// Allocation-efficient variant of [`Self::search_with_options`].
    pub fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        self.with_term(|term| search_term_buffer_shared(term, query, options))
    }

    /// The OSC 8 hyperlink under the given viewport cell, if any, expanded to
    /// the contiguous same-link run on that row.
    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<crate::links::DetectedLink> {
        self.with_term(|term| backend::hyperlink_at(term, row, col))
    }

    /// The OSC 8 or detected text link under the given viewport cell,
    /// including links spanning soft-wrapped rows.
    pub fn link_at(&self, row: usize, col: usize) -> Option<crate::links::DetectedViewportLink> {
        self.with_term(|term| backend::link_at(term, row, col))
    }

    fn with_term<R>(&self, f: impl FnOnce(&Term<JsonEventListener>) -> R) -> R {
        let term = self.term.lock();
        f(&term)
    }

    /// Access the terminal for in-place mutation.
    fn with_term_mut<R>(&self, f: impl FnOnce(&mut Term<JsonEventListener>) -> R) -> R {
        let mut term = self.term.lock();
        f(&mut term)
    }

    /// Consume and normalize terminal damage spans for incremental rendering.
    pub fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        self.with_term_mut(backend::take_damage_snapshot)
    }

    /// Scroll the displayed viewport through scrollback history.
    /// Positive deltas move up into history, negative deltas move down toward live output.
    pub fn scroll_display(&self, delta_lines: i32) -> bool {
        let mut term = self.term.lock();
        let changed = backend::scroll_display(&mut term, delta_lines);
        if changed {
            self.record_render_mutation(&term);
        }
        changed
    }

    /// Scroll the displayed viewport to the bottom (live output) atomically.
    /// Returns true if the scroll position changed.
    pub fn scroll_to_bottom(&self) -> bool {
        let mut term = self.term.lock();
        let changed = backend::scroll_to_bottom(&mut term);
        if changed {
            self.record_render_mutation(&term);
        }
        changed
    }

    /// Purge scrollback history and snap the viewport back to live output.
    /// Returns true if there was any history or scroll offset to clear.
    pub fn clear_scrollback(&self) -> bool {
        let mut term = self.term.lock();
        let changed = backend::clear_scrollback(&mut term);
        if changed {
            self.record_render_mutation(&term);
        }
        changed
    }

    /// Return `(display_offset, history_size)` for viewport scrollbar rendering.
    pub fn scroll_state(&self) -> (usize, usize) {
        let term = self.term.lock();
        backend::scroll_state(&term)
    }

    /// Get the cursor state the terminal currently intends to render.
    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        let term = self.term.lock();
        backend::cursor_state(&term)
    }

    /// Returns the cursor position regardless of visibility (for IME positioning).
    pub fn cursor_position(&self) -> (usize, usize) {
        let term = self.term.lock();
        backend::cursor_position(&term)
    }

    /// Check if there are pending events
    #[allow(dead_code)]
    pub fn has_pending_events(&self) -> bool {
        !self.events_rx.is_empty()
    }

    /// Sync live term options derived from the current runtime configuration.
    pub fn set_term_options(&self, options: TerminalOptions) {
        let mut term = self.term.lock();
        backend::apply_term_options(&mut term, options);
        self.record_render_mutation(&term);
    }

    /// Change only the live scrollback cap, preserving cursor defaults.
    pub fn set_scrollback_history(&self, scrollback_history: usize) {
        self.set_term_options(TerminalOptions {
            scrollback_history,
            default_cursor_style: self.default_cursor_style,
        });
    }

    /// Check if bracketed paste mode is enabled
    pub fn bracketed_paste_mode(&self) -> bool {
        let term = self.term.lock();
        backend::bracketed_paste_mode(&term)
    }

    /// Return current xterm mouse-reporting mode bits.
    pub fn mouse_mode(&self) -> TerminalMouseMode {
        let term = self.term.lock();
        backend::mouse_mode(*term.mode())
    }

    pub fn keyboard_mode(&self) -> TerminalKeyboardMode {
        let term = self.term.lock();
        backend::keyboard_mode(*term.mode())
    }

    /// Check if the terminal is currently in alternate screen mode
    pub fn alternate_screen_mode(&self) -> bool {
        let term = self.term.lock();
        backend::alternate_screen_mode(&term)
    }

    #[cfg(test)]
    pub(super) fn send_wakeup_for_test(&self) {
        self.listener.send_event(AlacEvent::Wakeup);
    }

    #[cfg(test)]
    pub(super) fn try_recv_event_for_test(&self) -> Option<RuntimeEvent> {
        self.events_rx.try_recv().ok()
    }

    #[cfg(test)]
    pub(super) fn event_queue_is_empty_for_test(&self) -> bool {
        self.events_rx.is_empty()
    }
}

impl Drop for AlacrittyBackend {
    fn drop(&mut self) {
        // Ensure the PTY event loop exits so PTY drop can terminate and
        // reap the child process.
        if let Some(pty_tx) = &self.pty_tx {
            let _ = pty_tx.send(EventLoopMsg::Shutdown);
        }
    }
}
