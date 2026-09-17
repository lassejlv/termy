use alacritty_terminal::{
    event::VoidListener,
    grid::{Dimensions, Scroll},
    sync::FairMutex,
    term::{Term, TermMode},
    vte::ansi,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use termy_core::{
    KittyClipboardControl, KittyClipboardHostState, KittyClipboardInput, KittyClipboardInterceptor,
    KittyClipboardOsc, KittyGraphicsInterceptor, KittyGraphicsItem, KittyGraphicsRenderPlacement,
    KittyGraphicsScreen, KittyGraphicsState, TerminalClipboardLocation, TerminalReplyHost,
    kitty_graphics_placeholders_from_alacritty_grid,
};

use termy_core::{
    DetectedLink, DetectedViewportLink, KittyGraphicsCursorTracker, TerminalCursorState,
    TerminalDamageSnapshot, TerminalKeyboardMode, TerminalMouseMode, TerminalOptions, TerminalSize,
};

use crate::terminal_ui::alacritty_bridge;

struct PaneTerminalInner {
    term: Arc<FairMutex<Term<VoidListener>>>,
    size: TerminalSize,
}

enum KittyClipboardPaneEvent {
    Packet(KittyClipboardOsc),
    SetPasteEvents(bool),
    Reset,
    Reply(Vec<u8>),
}

const MAX_KITTY_CLIPBOARD_EVENTS: usize = 65_536;

/// In-memory terminal emulator for a tmux pane.
pub struct PaneTerminal {
    inner: FairMutex<PaneTerminalInner>,
    parser: FairMutex<ansi::Processor>,
    kitty_clipboard_interceptor: FairMutex<KittyClipboardInterceptor>,
    kitty_clipboard: FairMutex<KittyClipboardHostState>,
    kitty_clipboard_events: FairMutex<VecDeque<KittyClipboardPaneEvent>>,
    kitty_clipboard_paste_events_enabled: AtomicBool,
    kitty_graphics_interceptor: FairMutex<KittyGraphicsInterceptor>,
    kitty_graphics_cursor_tracker: FairMutex<KittyGraphicsCursorTracker>,
    kitty_graphics: FairMutex<KittyGraphicsState>,
    kitty_graphics_revision: AtomicU64,
}

impl PaneTerminal {
    fn normalized_size(size: TerminalSize) -> TerminalSize {
        TerminalSize {
            cols: size.cols.max(1),
            rows: size.rows.max(1),
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        }
    }

    pub fn new(size: TerminalSize, options: TerminalOptions) -> Self {
        let size = Self::normalized_size(size);
        let config = alacritty_bridge::term_config(options);

        let term = Arc::new(FairMutex::new(Term::new(config, &size, VoidListener)));
        Self {
            inner: FairMutex::new(PaneTerminalInner { term, size }),
            parser: FairMutex::new(ansi::Processor::new()),
            kitty_clipboard_interceptor: FairMutex::new(KittyClipboardInterceptor::default()),
            kitty_clipboard: FairMutex::new(KittyClipboardHostState::new()),
            kitty_clipboard_events: FairMutex::new(VecDeque::new()),
            kitty_clipboard_paste_events_enabled: AtomicBool::new(false),
            kitty_graphics_interceptor: FairMutex::new(KittyGraphicsInterceptor::default()),
            kitty_graphics_cursor_tracker: FairMutex::new(KittyGraphicsCursorTracker::default()),
            kitty_graphics: FairMutex::new(KittyGraphicsState::default()),
            kitty_graphics_revision: AtomicU64::new(0),
        }
    }

    fn cloned_term_arc(&self) -> Arc<FairMutex<Term<VoidListener>>> {
        let inner = self.inner.lock();
        inner.term.clone()
    }

    pub fn feed_output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let (filtered, inputs) = self.kitty_clipboard_interceptor.lock().process(bytes);
        for input in inputs {
            match input {
                KittyClipboardInput::Packet(packet) => {
                    self.push_kitty_clipboard_event(KittyClipboardPaneEvent::Packet(packet));
                }
                KittyClipboardInput::Control(KittyClipboardControl::Set(enabled)) => {
                    self.kitty_clipboard_paste_events_enabled
                        .store(enabled, Ordering::Release);
                    self.push_kitty_clipboard_event(KittyClipboardPaneEvent::SetPasteEvents(
                        enabled,
                    ));
                }
                KittyClipboardInput::Control(KittyClipboardControl::Query) => {
                    let status = if self.kitty_clipboard_paste_events_enabled() {
                        1
                    } else {
                        2
                    };
                    self.push_kitty_clipboard_event(KittyClipboardPaneEvent::Reply(
                        format!("\x1b[?5522;{status}$y").into_bytes(),
                    ));
                }
                KittyClipboardInput::Control(KittyClipboardControl::Reset) => {
                    self.kitty_clipboard_paste_events_enabled
                        .store(false, Ordering::Release);
                    self.push_kitty_clipboard_event(KittyClipboardPaneEvent::Reset);
                }
            }
        }
        let mut interceptor = self.kitty_graphics_interceptor.lock();
        let mut cursor_tracker = self.kitty_graphics_cursor_tracker.lock();
        let mut parser = self.parser.lock();
        let term = self.cloned_term_arc();
        let mut term = term.lock();
        let mut graphics_changed = false;
        for item in interceptor.process(&filtered) {
            match item {
                KittyGraphicsItem::Text(text) => {
                    let mut graphics = self.kitty_graphics.lock();
                    let track_scrolls = graphics.has_placements();
                    let tracks_placeholders = graphics.has_virtual_placements();
                    graphics_changed |= alacritty_bridge::advance_graphics_text(
                        &mut cursor_tracker,
                        &mut parser,
                        &mut term,
                        &text,
                        track_scrolls,
                        &mut graphics,
                    );
                    graphics_changed |= tracks_placeholders;
                }
                KittyGraphicsItem::Command(command) => {
                    let cursor = term.grid().cursor.point;
                    let full_screen_scroll_region =
                        cursor_tracker.region_covers_full_screen(term.grid().screen_lines());
                    let screen = KittyGraphicsScreen::from_alternate_screen(
                        term.mode().contains(TermMode::ALT_SCREEN),
                    );
                    let mut graphics = self.kitty_graphics.lock();
                    let placeholders =
                        if command.action() == 'd' && graphics.has_virtual_placements() {
                            kitty_graphics_placeholders_from_alacritty_grid(term.grid())
                        } else {
                            Vec::new()
                        };
                    let result = graphics.apply_on_screen_with_placeholders(
                        command,
                        (cursor.column.0, cursor.line.0.max(0) as usize),
                        term.grid().history_size(),
                        self.size(),
                        screen,
                        &placeholders,
                    );
                    drop(graphics);
                    graphics_changed |= result.changed;
                    if result.cursor_advance_screen == Some(screen)
                        && let Some((cols, rows)) = result.cursor_advance
                    {
                        let untracked_scroll = alacritty_bridge::advance_graphics_cursor(
                            &mut term,
                            cols,
                            rows,
                            full_screen_scroll_region,
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
    }

    pub fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        self.kitty_clipboard_paste_events_enabled
            .load(Ordering::Acquire)
    }

    pub fn kitty_clipboard_paste_notification(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> Option<Vec<u8>> {
        let enabled = self.kitty_clipboard_paste_events_enabled();
        let mut state = self.kitty_clipboard.lock();
        state.set_paste_events_enabled(enabled);
        state.paste_notification(location, available_formats)
    }

    pub fn drain_kitty_clipboard_events(&self, host: &mut impl TerminalReplyHost) -> Vec<Vec<u8>> {
        let events = self
            .kitty_clipboard_events
            .lock()
            .drain(..)
            .collect::<Vec<_>>();
        let mut state = self.kitty_clipboard.lock();
        let mut replies = Vec::new();
        for event in events {
            match event {
                KittyClipboardPaneEvent::Packet(packet) => {
                    replies.extend(state.handle_osc(packet, host));
                }
                KittyClipboardPaneEvent::SetPasteEvents(enabled) => {
                    state.set_paste_events_enabled(enabled);
                }
                KittyClipboardPaneEvent::Reset => state.reset(),
                KittyClipboardPaneEvent::Reply(reply) => replies.push(reply),
            }
        }
        replies
    }

    fn push_kitty_clipboard_event(&self, event: KittyClipboardPaneEvent) {
        let mut events = self.kitty_clipboard_events.lock();
        if events.len() < MAX_KITTY_CLIPBOARD_EVENTS {
            events.push_back(event);
        }
    }

    pub fn resize(&self, new_size: TerminalSize) {
        let new_size = Self::normalized_size(new_size);
        let mut cursor_tracker = self.kitty_graphics_cursor_tracker.lock();
        let term = self.cloned_term_arc();
        let mut term = term.lock();
        let mut inner = self.inner.lock();
        // Keep cached pane size and the backing terminal dimensions synchronized
        // under the same critical section so concurrent readers never observe
        // a partially-applied resize.
        inner.size = new_size;
        term.resize(new_size);
        cursor_tracker.reset_scroll_region();
        self.kitty_graphics.lock().resize(new_size);
        self.kitty_graphics_revision.fetch_add(1, Ordering::Relaxed);
    }

    pub fn size(&self) -> TerminalSize {
        self.inner.lock().size
    }

    pub fn with_term<R>(&self, f: impl FnOnce(&Term<VoidListener>) -> R) -> R {
        let term = self.cloned_term_arc();
        // Run callback outside the outer state lock so callbacks can safely call
        // back into PaneTerminal APIs (for example size()) without lock inversion.
        let term = term.lock();
        f(&term)
    }

    fn with_term_mut<R>(
        &self,
        f: impl FnOnce(&mut Term<VoidListener>, &mut PaneTerminalInner) -> R,
    ) -> R {
        let term = self.cloned_term_arc();
        let mut term = term.lock();
        let mut inner = self.inner.lock();
        let result = f(&mut term, &mut inner);
        // Keep cached dimensions aligned with any in-place terminal mutation so
        // PaneTerminalInner and Term cannot drift.
        inner.size.cols = u16::try_from(term.grid().columns()).unwrap_or(u16::MAX);
        inner.size.rows = u16::try_from(term.grid().screen_lines()).unwrap_or(u16::MAX);
        result
    }

    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<DetectedLink> {
        let term = self.cloned_term_arc();
        alacritty_bridge::hyperlink_at(&term.lock(), row, col)
    }

    pub fn link_at(&self, row: usize, col: usize) -> Option<DetectedViewportLink> {
        let term = self.cloned_term_arc();
        alacritty_bridge::link_at(&term.lock(), row, col)
    }

    pub fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        self.with_term_mut(|term, _inner| alacritty_bridge::take_damage_snapshot(term))
    }

    pub fn scroll_display(&self, delta_lines: i32) -> bool {
        if delta_lines == 0 {
            return false;
        }

        let term = self.cloned_term_arc();
        let mut term = term.lock();
        let old_offset = term.grid().display_offset();
        term.scroll_display(Scroll::Delta(delta_lines));
        term.grid().display_offset() != old_offset
    }

    pub fn scroll_to_bottom(&self) -> bool {
        let term = self.cloned_term_arc();
        let mut term = term.lock();
        let old_offset = term.grid().display_offset();
        if old_offset == 0 {
            return false;
        }
        term.scroll_display(Scroll::Bottom);
        true
    }

    pub fn scroll_state(&self) -> (usize, usize) {
        let term = self.cloned_term_arc();
        let term = term.lock();
        let grid = term.grid();
        (grid.display_offset(), grid.history_size())
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        let term = self.cloned_term_arc();
        let term = term.lock();
        alacritty_bridge::cursor_state(&term)
    }

    /// Returns the cursor position regardless of visibility (for IME positioning).
    pub fn cursor_position(&self) -> (usize, usize) {
        let term = self.cloned_term_arc();
        let term = term.lock();
        alacritty_bridge::cursor_position(&term)
    }

    pub fn set_term_options(&self, options: TerminalOptions) {
        self.with_term_mut(|term, _inner| term.set_options(alacritty_bridge::term_config(options)));
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        let term = self.cloned_term_arc();
        term.lock().mode().contains(TermMode::BRACKETED_PASTE)
    }

    pub fn mouse_mode(&self) -> TerminalMouseMode {
        let term = self.cloned_term_arc();
        let mode = *term.lock().mode();
        alacritty_bridge::mouse_mode(mode)
    }

    pub fn keyboard_mode(&self) -> TerminalKeyboardMode {
        let term = self.cloned_term_arc();
        alacritty_bridge::keyboard_mode(*term.lock().mode())
    }

    pub fn alternate_screen_mode(&self) -> bool {
        let term = self.cloned_term_arc();
        term.lock().mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.kitty_graphics_snapshot().1
    }

    pub fn kitty_graphics_revision(&self) -> u64 {
        if self
            .kitty_graphics
            .lock()
            .advance_animations(std::time::Instant::now())
        {
            self.kitty_graphics_revision.fetch_add(1, Ordering::Relaxed);
        }
        self.kitty_graphics_revision.load(Ordering::Relaxed)
    }

    pub fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        let revision = self.kitty_graphics_revision();
        let term = self.cloned_term_arc();
        let term = term.lock();
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
        (revision, placements)
    }
}

impl Dimensions for PaneTerminal {
    fn total_lines(&self) -> usize {
        self.size().rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.size().rows as usize
    }

    fn columns(&self) -> usize {
        self.size().cols as usize
    }

    fn last_column(&self) -> alacritty_terminal::index::Column {
        alacritty_terminal::index::Column(self.size().cols.saturating_sub(1) as usize)
    }

    fn bottommost_line(&self) -> alacritty_terminal::index::Line {
        alacritty_terminal::index::Line(i32::from(self.size().rows.saturating_sub(1)))
    }

    fn topmost_line(&self) -> alacritty_terminal::index::Line {
        alacritty_terminal::index::Line(0)
    }
}

#[cfg(test)]
mod tests {
    use super::PaneTerminal;
    use alacritty_terminal::grid::Dimensions;
    use termy_core::{TerminalClipboardLocation, TerminalOptions, TerminalSize};

    fn test_term_options(scrollback_history: usize) -> TerminalOptions {
        TerminalOptions {
            scrollback_history,
            ..TerminalOptions::default()
        }
    }

    fn visible_viewport_text(terminal: &PaneTerminal) -> String {
        let size = terminal.size();
        let cols = size.cols as usize;
        let rows = size.rows as usize;
        let mut grid = vec![vec![' '; cols]; rows];

        terminal.with_term(|term| {
            let content = term.renderable_content();
            for cell in content.display_iter {
                let row = cell.point.line.0 + content.display_offset as i32;
                if row < 0 {
                    continue;
                }
                let row = row as usize;
                let col = cell.point.column.0;
                if row >= rows || col >= cols {
                    continue;
                }

                let c = cell.cell.c;
                if c != '\0' && !c.is_control() {
                    grid[row][col] = c;
                }
            }
        });

        grid.into_iter()
            .map(|row| {
                let line: String = row.into_iter().collect();
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn feed_output_handles_tmux_prompt_repaint_without_prefix_duplication() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 120,
                rows: 10,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.feed_output(
            b"c\x08cd Desk\x08\x08\x08\x08\x08\x08\x08\x1b[32mc\x1b[32md\x1b[39m\x1b[5C\r\r\n",
        );
        terminal.feed_output(b"cd: no such file or directory: Desk\r\n");
        terminal.feed_output(b"c\x08\x1b[4mc\x1b[24m\r\r\n");
        terminal.feed_output(b"zsh: command not found: c\r\n");

        let visible = visible_viewport_text(&terminal);
        assert!(visible.contains("cd: no such file or directory: Desk"));
        assert!(visible.contains("zsh: command not found: c"));
        assert!(!visible.contains("cdcd:"));
        assert!(!visible.contains("czsh:"));
    }

    #[test]
    fn clamps_zero_size_on_new_and_resize() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 0,
                rows: 0,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );
        assert_eq!(terminal.size().cols, 1);
        assert_eq!(terminal.size().rows, 1);
        assert_eq!(terminal.last_column().0, 0);
        assert_eq!(terminal.bottommost_line().0, 0);

        terminal.resize(TerminalSize {
            cols: 0,
            rows: 0,
            ..TerminalSize::default()
        });
        assert_eq!(terminal.size().cols, 1);
        assert_eq!(terminal.size().rows, 1);
        assert_eq!(terminal.last_column().0, 0);
        assert_eq!(terminal.bottommost_line().0, 0);
    }

    #[test]
    fn with_term_mut_resyncs_cached_size_after_direct_term_resize() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 4,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.with_term_mut(|term, _inner| {
            term.resize(TerminalSize {
                cols: 9,
                rows: 7,
                ..TerminalSize::default()
            });
        });

        let size = terminal.size();
        assert_eq!(size.cols, 9);
        assert_eq!(size.rows, 7);
    }

    #[test]
    fn mouse_mode_detects_click_and_sgr_flags_from_output_stream() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 4,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b[?1000h\x1b[?1006h");
        let mode = terminal.mouse_mode();
        assert!(mode.enabled);
        assert!(mode.report_click);
        assert!(mode.sgr_encoding);
        assert!(!mode.report_drag);
        assert!(!mode.report_motion);
    }

    #[test]
    fn mouse_mode_detects_drag_and_motion_flags_from_output_stream() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 4,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b[?1002h");
        let drag_mode = terminal.mouse_mode();
        assert!(drag_mode.enabled);
        assert!(drag_mode.report_drag);
        assert!(!drag_mode.report_motion);

        terminal.feed_output(b"\x1b[?1003h");
        let motion_mode = terminal.mouse_mode();
        assert!(motion_mode.enabled);
        assert!(motion_mode.report_motion);
    }

    #[test]
    fn tmux_link_lookup_is_owned_by_pane_terminal() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 12,
                rows: 3,
                ..TerminalSize::default()
            },
            test_term_options(2000),
        );
        terminal.feed_output(
            b"\x1b]8;id=docs;https://example.com/docs\x1b\\docs\x1b]8;;\x1b\\ https://x.io",
        );

        let hyperlink = terminal.hyperlink_at(0, 1).expect("OSC 8 link");
        assert_eq!((hyperlink.start_col, hyperlink.end_col), (0, 3));
        assert_eq!(hyperlink.target, "https://example.com/docs");

        let detected = terminal.link_at(1, 3).expect("wrapped text link");
        assert_eq!(detected.target, "https://x.io");
    }

    #[test]
    fn feed_output_intercepts_kitty_graphics_for_tmux_panes() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=91,c=2,r=2;AQID/w==\x1b\\");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 91);
        assert_eq!(placements[0].display_cols, Some(2));
        assert_eq!(terminal.cursor_position(), (2, 2));
    }

    #[test]
    fn direct_kitty_image_does_not_churn_revision_for_unrelated_text() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=91,c=2,r=2,C=1;AQID/w==\x1b\\");
        let revision = terminal.kitty_graphics_revision();

        terminal.feed_output(b"ordinary text");

        assert_eq!(terminal.kitty_graphics_revision(), revision);
    }

    #[test]
    fn tmux_kitty_relative_image_tracks_and_clears_with_unicode_placeholder() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(
            b"\x1b_Ga=t,f=32,s=1,v=1,i=41,q=1;AQID/w==\x1b\\\
              \x1b_Ga=t,f=32,s=1,v=1,i=42,q=1;AQID/w==\x1b\\\
              \x1b_Ga=p,U=1,i=41,p=7,c=1,r=1,C=1,q=1;\x1b\\\
              \x1b_Ga=p,i=42,p=8,P=41,Q=7,H=3,V=-2,c=2,r=2,C=1,q=1;\x1b\\\
              \x1b[7;5H\x1b[38;5;41;58;5;7m",
        );
        terminal.feed_output("\u{10eeee}\u{0305}\u{0305}".as_bytes());
        terminal.feed_output(b"\x1b[0m");

        let child = terminal
            .kitty_graphics_placements()
            .into_iter()
            .find(|placement| placement.image_id == 42)
            .expect("relative child should follow the placeholder");
        assert_eq!((child.viewport_row, child.col), (4, 7));

        let revision = terminal.kitty_graphics_revision();
        terminal.feed_output(b"\x1b[7;5Hx");
        assert!(terminal.kitty_graphics_placements().is_empty());
        assert!(terminal.kitty_graphics_revision() > revision);
    }

    #[test]
    fn clear_screen_removes_kitty_graphics_from_tmux_panes() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 10,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(b"\x1b_Ga=T,f=32,s=1,v=1,i=96,c=2,r=2,C=1;AQID/w==\x1b\\");
        assert_eq!(terminal.kitty_graphics_placements().len(), 1);

        terminal.feed_output(b"\x1b[H\x1b[2J");

        assert!(terminal.kitty_graphics_placements().is_empty());
    }

    #[test]
    fn kitty_cursor_advance_scrolls_tmux_pane_at_bottom() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal.feed_output(b"\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=92,c=2,r=3;AQID/w==\x1b\\");

        assert_eq!(terminal.scroll_state(), (0, 3));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_tracks_tmux_alternate_screen_scroll() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal
            .feed_output(b"\x1b[?1049h\x1b[4;1H\x1b_Ga=T,f=32,s=1,v=1,i=93,c=2,r=3;AQID/w==\x1b\\");

        assert!(terminal.alternate_screen_mode());
        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (2, 3));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn ordinary_newlines_shift_tmux_alternate_screen_kitty_placement() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );
        terminal.feed_output(
            b"\x1b[?1049h\x1b[2;1H\x1b_Ga=T,f=32,s=1,v=1,i=95,c=2,r=1,C=1;AQID/w==\x1b\\",
        );

        terminal.feed_output(b"\x1b[4;1H\n");

        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].viewport_row, 0);
    }

    #[test]
    fn kitty_cursor_advance_does_not_scroll_tmux_partial_region() {
        let terminal = PaneTerminal::new(
            TerminalSize {
                cols: 20,
                rows: 4,
                cell_width: 10.0,
                cell_height: 20.0,
            },
            test_term_options(2000),
        );

        terminal.feed_output(
            b"\x1b[2;3r\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=94,c=2,r=3;AQID/w==\x1b\\\x1b[r",
        );

        assert_eq!(terminal.scroll_state(), (0, 0));
        assert_eq!(terminal.cursor_position(), (0, 0));
        let placements = terminal.kitty_graphics_placements();
        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].image_id, 94);
        assert_eq!(placements[0].viewport_row, 2);
    }

    #[test]
    fn tmux_panes_route_kitty_clipboard_packets_and_paste_mode() {
        let terminal = PaneTerminal::new(TerminalSize::default(), test_term_options(100));
        terminal.feed_output(
            b"\x1b[?5522h\x1b[?5522$p\x1b]5522;type=write:id=tmux\x1b\\\
              \x1b]5522;type=wdata\x1b\\",
        );

        assert!(terminal.kitty_clipboard_paste_events_enabled());
        let replies = terminal.drain_kitty_clipboard_events(&mut |_| None);
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0], b"\x1b[?5522;1$y");
        assert_eq!(
            replies[1],
            b"\x1b]5522;type=write:status=ENOSYS:id=tmux\x1b\\"
        );

        let notification = terminal
            .kitty_clipboard_paste_notification(
                TerminalClipboardLocation::Clipboard,
                &["text/plain".to_string(), "image/png".to_string()],
            )
            .unwrap();
        assert!(String::from_utf8_lossy(&notification).contains("status=DATA:mime=Lg=="));
    }
}
