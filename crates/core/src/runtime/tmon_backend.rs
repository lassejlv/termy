use super::*;
use crate::{
    DetectedLink, DetectedViewportLink, TerminalClipboardTarget, TerminalColor, TerminalPalette,
    TerminalRenderCell, TerminalRenderColor, TerminalRenderText, TerminalUnderlineStyle,
    TerminalViewportScroll, TerminalViewportScrollDirection, TermyCell, TermyColor,
    find_link_in_line,
};

pub(super) struct TmonBackend {
    terminal: tmon::Terminal,
    kitty_clipboard: std::sync::Mutex<KittyClipboardHostState>,
    query_colors: TerminalQueryColors,
    default_cursor_style: TerminalCursorStyle,
    last_damage_cursor: std::sync::Mutex<Option<tmon::CursorState>>,
}

impl TmonBackend {
    pub(super) fn new(
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

    pub(super) fn new_with_wakeup_notifier(
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

    pub(super) fn new_with_launch_and_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
    ) -> anyhow::Result<Self> {
        let runtime_config = runtime_config.cloned().unwrap_or_default();
        let config = native_config(
            configured_working_dir,
            tab_title_shell_integration,
            &runtime_config,
            launch,
        )?;
        let wakeup_notifier =
            wakeup_notifier.map(|notifier| tmon::WakeupNotifier::new(move || notifier.notify()));
        let terminal = tmon::Terminal::new(tmon_size(size.clamped()), config, wakeup_notifier)?;
        let last_damage_cursor = std::sync::Mutex::new(terminal.cursor_state());
        Ok(Self {
            terminal,
            kitty_clipboard: std::sync::Mutex::new(KittyClipboardHostState::new()),
            query_colors: runtime_config.query_colors,
            default_cursor_style: runtime_config.default_cursor_style,
            last_damage_cursor,
        })
    }

    pub(super) fn new_display_with_wakeup_notifier(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        let runtime_config = runtime_config.cloned().unwrap_or_default();
        let size = tmon_size(size.clamped());
        let config = display_config(&runtime_config);
        let terminal = match wakeup_notifier {
            Some(notifier) => tmon::Terminal::new_display_with_wakeup_notifier(
                size,
                config,
                tmon::WakeupNotifier::new(move || notifier.notify()),
            ),
            None => tmon::Terminal::new_display(size, config),
        };
        let last_damage_cursor = std::sync::Mutex::new(terminal.cursor_state());
        Self {
            terminal,
            kitty_clipboard: std::sync::Mutex::new(KittyClipboardHostState::new()),
            query_colors: runtime_config.query_colors,
            default_cursor_style: runtime_config.default_cursor_style,
            last_damage_cursor,
        }
    }

    pub(super) fn feed_output(&self, bytes: &[u8]) {
        self.terminal.feed_output(bytes);
    }

    pub(super) fn child_pid(&self) -> Option<u32> {
        self.terminal.child_pid()
    }

    pub(super) fn set_wakeup_enabled(&self, enabled: bool) {
        self.terminal.set_wakeup_enabled(enabled);
    }

    pub(super) fn write(&self, input: &[u8]) {
        self.terminal.write(input);
    }

    pub(super) fn write_owned(&self, input: Vec<u8>) {
        self.terminal.write_owned(input);
    }

    pub(super) fn hydrate_output(&self, bytes: &[u8]) {
        self.terminal.hydrate_output(bytes);
    }

    pub(super) fn write_str(&self, input: &str) {
        self.write(input.as_bytes());
    }

    pub(super) fn resize(&mut self, new_size: TerminalSize) {
        self.terminal.resize(tmon_size(new_size.clamped()));
    }

    pub(super) fn nudge_resize(&self) {
        self.terminal.nudge_resize();
    }

    pub(super) fn size(&self) -> TerminalSize {
        terminal_size(self.terminal.size())
    }

    pub(super) fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.terminal
            .kitty_graphics_placements()
            .into_iter()
            .map(graphics_placement)
            .collect()
    }

    pub(super) fn kitty_graphics_revision(&self) -> u64 {
        self.terminal.kitty_graphics_revision()
    }

    pub(super) fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        let (revision, placements) = self.terminal.kitty_graphics_snapshot();
        (
            revision,
            placements.into_iter().map(graphics_placement).collect(),
        )
    }

    pub(super) fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        self.terminal.kitty_clipboard_paste_events_mode()
    }

    pub(super) fn send_kitty_clipboard_paste_event(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> bool {
        let enabled = self.terminal.kitty_clipboard_paste_events_mode();
        let mut state = self
            .kitty_clipboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.set_paste_events_enabled(enabled);
        let Some(notification) = state.paste_notification(location, available_formats) else {
            return false;
        };
        drop(state);
        self.terminal.write_protocol_reply_owned(notification);
        true
    }

    pub(super) fn drain_events(
        &self,
        host: &mut impl TerminalReplyHost,
    ) -> (Vec<TerminalEvent>, bool) {
        let (events, has_more) = self.terminal.drain_events();
        let mut translated = Vec::with_capacity(events.len());
        for event in events {
            match event {
                tmon::Event::ClipboardLoad(request) => {
                    if let Some(text) = host.load_clipboard(clipboard_target(request.target())) {
                        self.terminal
                            .write_protocol_reply_owned(request.format_reply(&text));
                    }
                }
                tmon::Event::KittyClipboard(packet) => {
                    let terminator = if packet.bell_terminated() {
                        KittyClipboardOscTerminator::Bell
                    } else {
                        KittyClipboardOscTerminator::StringTerminator
                    };
                    let packet = KittyClipboardOsc::from_body(packet.body(), terminator);
                    let mut state = self
                        .kitty_clipboard
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let responses = state.handle_osc(packet, host);
                    drop(state);
                    for response in responses {
                        self.terminal.write_protocol_reply_owned(response);
                    }
                }
                tmon::Event::KittyClipboardMode(enabled) => self
                    .kitty_clipboard
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_paste_events_enabled(enabled),
                tmon::Event::KittyClipboardReset => self
                    .kitty_clipboard
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .reset(),
                event => translated.push(terminal_event(event)),
            }
        }
        let protocol_replies = self.terminal.drain_protocol_replies();
        if !protocol_replies.is_empty() {
            host.protocol_reply(&protocol_replies);
        }
        (translated, has_more)
    }

    pub(super) fn set_query_colors(&mut self, query_colors: TerminalQueryColors) {
        self.query_colors = query_colors;
        self.terminal
            .set_query_colors(tmon_query_colors(query_colors));
    }

    pub(super) fn palette(&self) -> TerminalPalette {
        terminal_palette(&self.terminal.palette())
    }

    pub(super) fn snapshot(&self) -> TermyFrame {
        let (snapshot, palette) = self.terminal.snapshot_with_palette();
        TermyFrame {
            cols: snapshot.cols,
            rows: snapshot.rows,
            cells: snapshot
                .cells
                .iter()
                .map(|cell| legacy_cell(cell, &palette, self.query_colors))
                .collect(),
            cursor: snapshot.cursor.map(cursor_state),
            display_offset: snapshot.display_offset,
            history_size: snapshot.history_size,
        }
    }

    pub(super) fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        let mut source_cells = Vec::new();
        let update = self
            .terminal
            .visit_render_read(force_full, |_, _, _, cell, _| source_cells.push(*cell));
        let damage = if update.render.scrolls.is_empty() {
            self.damage_with_cursor(damage(update.render.damage), update.viewport)
        } else {
            self.damage_with_cursor(TerminalDamageSnapshot::Full, update.viewport)
        };
        let cells = legacy_update_cells(
            &source_cells,
            &damage,
            usize::from(update.size.cols),
            &update.palette,
            self.query_colors,
        );
        TermyFrameUpdate {
            cols: update.size.cols,
            rows: update.size.rows,
            cells,
            cursor: update.viewport.cursor.map(cursor_state),
            display_offset: update.viewport.display_offset,
            history_size: update.viewport.history_size,
            damage,
        }
    }

    pub(super) fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        let (update, palette_revision, viewport) = self
            .terminal
            .take_render_damage_snapshot_with_render_state();
        let mut update = render_damage(update, palette_revision);
        update.damage = self.damage_with_cursor(update.damage, viewport);
        update
    }

    pub(super) fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        let mut cells = Vec::new();
        let update = self
            .terminal
            .visit_render_read(force_full, |_, _, _, cell, combining| {
                cells.push(render_cell(cell, combining));
            });
        let palette = terminal_palette(&update.palette);
        let mut render_update = render_damage(update.render, palette.revision);
        render_update.damage = self.damage_with_cursor(render_update.damage, update.viewport);
        TerminalRenderRead {
            metadata: TerminalViewportMetadata {
                cols: update.viewport.cols,
                rows: update.viewport.rows,
                cursor: update.viewport.cursor.map(cursor_state),
                display_offset: update.viewport.display_offset,
                history_size: update.viewport.history_size,
                palette_revision: palette.revision,
                generation: render_update.generation,
            },
            palette,
            cells,
            update: render_update,
        }
    }

    pub(super) fn visit_viewport_cells(
        &self,
        mut visitor: impl FnMut(usize, i32, usize, &TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        let mut cells = Vec::new();
        let (viewport, generation, palette_revision) =
            self.terminal.visit_viewport_cells_with_render_state(
                |display_offset, line, col, cell, combining| {
                    cells.push((display_offset, line, col, render_cell(cell, combining)));
                },
            );
        for (display_offset, line, col, cell) in &cells {
            visitor(*display_offset, *line, *col, cell);
        }
        TerminalViewportMetadata {
            cols: viewport.cols,
            rows: viewport.rows,
            cursor: viewport.cursor.map(cursor_state),
            display_offset: viewport.display_offset,
            history_size: viewport.history_size,
            palette_revision,
            generation,
        }
    }

    pub(super) fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        mut visitor: impl FnMut(usize, usize, i32, usize, &TerminalRenderCell),
    ) -> bool {
        let ranges = spans
            .iter()
            .map(|span| (span.row, span.left_col, span.right_col));
        let mut cells = Vec::new();
        let visited = self.terminal.for_each_viewport_range_at_generation(
            generation,
            ranges,
            |row, display_offset, line, col, cell, combining| {
                cells.push((row, display_offset, line, col, render_cell(cell, combining)));
            },
        );
        if visited.is_none() {
            return false;
        }
        for (row, display_offset, line, col, cell) in &cells {
            visitor(*row, *display_offset, *line, *col, cell);
        }
        true
    }

    pub(super) fn line_bounds(&self) -> (i32, i32) {
        self.terminal.line_bounds()
    }

    pub(super) fn visit_line_cells(
        &self,
        requested_first: i32,
        requested_last: i32,
        mut visitor: impl FnMut((i32, i32, usize), i32, usize, &TerminalRenderCell),
    ) -> (i32, i32, usize) {
        let range = self.terminal.for_each_line_cell_range(
            requested_first,
            requested_last,
            |range, line, col, cell, combining| {
                let cell = render_cell(cell, combining);
                visitor(
                    (range.first_line, range.last_line, range.columns),
                    line,
                    col,
                    &cell,
                );
            },
        );
        (range.first_line, range.last_line, range.columns)
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
        let (first_line, last_line) = self.terminal.line_bounds();
        let line_count = last_line
            .checked_sub(first_line)
            .and_then(|count| usize::try_from(count).ok())
            .and_then(|count| count.checked_add(1))
            .unwrap_or(0);
        let mut lines = vec![String::new(); line_count];
        self.terminal
            .for_each_line_cell_range(first_line, last_line, |_, line, _, cell, _| {
                let index = usize::try_from(line.saturating_sub(first_line)).unwrap_or(usize::MAX);
                if let Some(text) = lines.get_mut(index) {
                    text.push(search_character(cell));
                }
            });
        for line in &mut lines {
            line.truncate(line.trim_end().len());
        }
        search_lines_shared(lines.into_iter().enumerate(), query, options)
    }

    pub(super) fn hyperlink_at(&self, row: usize, col: usize) -> Option<DetectedLink> {
        self.terminal
            .hyperlink_at(row, col)
            .map(|link| DetectedLink {
                start_col: link.start_col,
                end_col: link.end_col,
                target: link.target,
            })
    }

    pub(super) fn link_at(&self, row: usize, col: usize) -> Option<DetectedViewportLink> {
        self.terminal
            .link_at(row, col, |characters, hovered_index| {
                find_link_in_line(characters, hovered_index).map(|link| tmon::LinkMatch {
                    start_col: link.start_col,
                    end_col: link.end_col,
                    target: link.target,
                })
            })
            .map(|link| DetectedViewportLink {
                start_row: link.start_row,
                start_col: link.start_col,
                end_row: link.end_row,
                end_col: link.end_col,
                target: link.target,
            })
    }

    pub(super) fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        let (raw_damage, viewport) = self.terminal.take_damage_snapshot_with_viewport_state();
        self.damage_with_cursor(damage(raw_damage), viewport)
    }

    pub(super) fn scroll_display(&self, delta_lines: i32) -> bool {
        self.terminal.scroll_display(delta_lines)
    }

    pub(super) fn scroll_to_bottom(&self) -> bool {
        self.terminal.scroll_to_bottom()
    }

    pub(super) fn clear_scrollback(&self) -> bool {
        self.terminal.clear_scrollback()
    }

    pub(super) fn scroll_state(&self) -> (usize, usize) {
        self.terminal.scroll_state()
    }

    pub(super) fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.terminal.cursor_state().map(cursor_state)
    }

    pub(super) fn cursor_position(&self) -> (usize, usize) {
        self.terminal.cursor_position()
    }

    pub(super) fn has_pending_events(&self) -> bool {
        self.terminal.has_pending_events()
    }

    pub(super) fn set_term_options(&self, options: TerminalOptions) {
        self.terminal.set_options(tmon_options(options));
    }

    pub(super) fn set_scrollback_history(&self, scrollback_history: usize) {
        self.set_term_options(TerminalOptions {
            scrollback_history,
            default_cursor_style: self.default_cursor_style,
        });
    }

    pub(super) fn bracketed_paste_mode(&self) -> bool {
        self.terminal.bracketed_paste_mode()
    }

    pub(super) fn mouse_mode(&self) -> TerminalMouseMode {
        let mode = self.terminal.mouse_mode();
        TerminalMouseMode {
            enabled: mode.enabled,
            report_click: mode.report_click,
            report_drag: mode.report_drag,
            report_motion: mode.report_motion,
            sgr_encoding: mode.sgr_encoding,
            utf8_encoding: mode.utf8_encoding,
        }
    }

    pub(super) fn keyboard_mode(&self) -> TerminalKeyboardMode {
        let mode = self.terminal.keyboard_mode();
        TerminalKeyboardMode::from_flags(
            mode.application_cursor_keys,
            mode.disambiguate_escape_codes,
            mode.report_event_types,
            mode.report_alternate_keys,
            mode.report_all_keys_as_esc,
            mode.report_associated_text,
        )
    }

    pub(super) fn alternate_screen_mode(&self) -> bool {
        self.terminal.alternate_screen_mode()
    }

    fn damage_with_cursor(
        &self,
        damage: TerminalDamageSnapshot,
        viewport: tmon::ViewportMetadata,
    ) -> TerminalDamageSnapshot {
        let mut previous = self
            .last_damage_cursor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = viewport.cursor;
        let old = *previous;
        *previous = current;
        let TerminalDamageSnapshot::Partial(mut spans) = damage else {
            return damage;
        };
        if old == current {
            return TerminalDamageSnapshot::Partial(spans);
        }
        let rows = usize::from(viewport.rows);
        let cols = usize::from(viewport.cols);
        for cursor in [old, current].into_iter().flatten() {
            let Some(row) = cursor.row.checked_add(viewport.display_offset) else {
                continue;
            };
            if row >= rows || cols == 0 {
                continue;
            }
            spans.push(TerminalDirtySpan {
                row,
                left_col: cursor.col.saturating_sub(1),
                right_col: cursor.col.saturating_add(1).min(cols - 1),
            });
        }
        normalize_damage_spans(&mut spans);
        TerminalDamageSnapshot::Partial(spans)
    }
}

fn native_config(
    configured_working_dir: Option<&str>,
    shell_integration: Option<&TabTitleShellIntegration>,
    runtime_config: &TerminalRuntimeConfig,
    launch: Option<&TerminalLaunch>,
) -> anyhow::Result<tmon::Config> {
    let resolved_launch = resolve_terminal_launch(runtime_config, launch)?;
    Ok(tmon::Config {
        shell: None,
        working_directory: resolve_launch_working_directory(
            configured_working_dir,
            runtime_config.working_dir_fallback,
        ),
        environment: terminal_environment_overrides(shell_integration, runtime_config)
            .into_iter()
            .collect(),
        launch: Some(tmon::Launch::Program {
            program: resolved_launch.program,
            args: resolved_launch.args,
        }),
        scrollback_history: runtime_config
            .scrollback_history
            .min(MAX_TERMINAL_SCROLLBACK_HISTORY),
        default_cursor_style: tmon_cursor_style(runtime_config.default_cursor_style),
        query_colors: tmon_query_colors(runtime_config.query_colors),
        osc52: tmon::Osc52::CopyPaste,
    })
}

fn display_config(runtime_config: &TerminalRuntimeConfig) -> tmon::Config {
    tmon::Config {
        scrollback_history: runtime_config
            .scrollback_history
            .min(MAX_TERMINAL_SCROLLBACK_HISTORY),
        default_cursor_style: tmon_cursor_style(runtime_config.default_cursor_style),
        query_colors: tmon_query_colors(runtime_config.query_colors),
        osc52: tmon::Osc52::CopyPaste,
        ..tmon::Config::default()
    }
}

fn tmon_size(size: TerminalSize) -> tmon::Size {
    tmon::Size {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

fn terminal_size(size: tmon::Size) -> TerminalSize {
    TerminalSize {
        cols: size.cols,
        rows: size.rows,
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

fn tmon_cursor_style(style: TerminalCursorStyle) -> tmon::CursorStyle {
    match style {
        TerminalCursorStyle::Line => tmon::CursorStyle::Line,
        TerminalCursorStyle::Block => tmon::CursorStyle::Block,
    }
}

fn cursor_state(state: tmon::CursorState) -> TerminalCursorState {
    TerminalCursorState {
        col: state.col,
        row: state.row,
        style: match state.style {
            tmon::CursorStyle::Line => TerminalCursorStyle::Line,
            tmon::CursorStyle::Block => TerminalCursorStyle::Block,
        },
    }
}

fn tmon_options(options: TerminalOptions) -> tmon::TerminalOptions {
    tmon::TerminalOptions {
        scrollback_history: options
            .scrollback_history
            .min(MAX_TERMINAL_SCROLLBACK_HISTORY),
        default_cursor_style: tmon_cursor_style(options.default_cursor_style),
    }
}

fn tmon_query_colors(colors: TerminalQueryColors) -> tmon::QueryColors {
    let rgb = |color: TerminalColor| tmon::Rgb {
        r: color.r,
        g: color.g,
        b: color.b,
    };
    tmon::QueryColors {
        ansi: colors.ansi.map(rgb),
        foreground: rgb(colors.foreground),
        background: rgb(colors.background),
        cursor: colors.cursor.map(rgb),
    }
}

fn terminal_event(event: tmon::Event) -> TerminalEvent {
    match event {
        tmon::Event::Wakeup => TerminalEvent::Wakeup,
        tmon::Event::Title(title) => TerminalEvent::Title(title),
        tmon::Event::ResetTitle => TerminalEvent::ResetTitle,
        tmon::Event::Bell => TerminalEvent::Bell,
        tmon::Event::Exit => TerminalEvent::Exit,
        tmon::Event::ClipboardStore(text) => TerminalEvent::ClipboardStore(text),
        tmon::Event::ClipboardLoad(_) => unreachable!("clipboard loads are handled before mapping"),
        tmon::Event::KittyClipboard(_)
        | tmon::Event::KittyClipboardMode(_)
        | tmon::Event::KittyClipboardReset => {
            unreachable!("Kitty clipboard events are handled before mapping")
        }
        tmon::Event::ShellPromptStart => TerminalEvent::ShellPromptStart,
        tmon::Event::ShellCommandStart => TerminalEvent::ShellCommandStart,
        tmon::Event::ShellCommandExecuting => TerminalEvent::ShellCommandExecuting,
        tmon::Event::ShellCommandFinished(exit_code) => {
            TerminalEvent::ShellCommandFinished(exit_code)
        }
        tmon::Event::Progress(progress) => TerminalEvent::Progress(match progress {
            tmon::Progress::Clear => ProgressState::Clear,
            tmon::Progress::InProgress(value) => ProgressState::InProgress(value),
            tmon::Progress::Error(value) => ProgressState::Error(value),
            tmon::Progress::Indeterminate => ProgressState::Indeterminate,
            tmon::Progress::Warning(value) => ProgressState::Warning(value),
        }),
        tmon::Event::WorkingDirectory(path) => TerminalEvent::WorkingDirectory(path),
    }
}

fn clipboard_target(target: tmon::ClipboardTarget) -> TerminalClipboardTarget {
    match target {
        tmon::ClipboardTarget::Clipboard => TerminalClipboardTarget::Clipboard,
        tmon::ClipboardTarget::Selection => TerminalClipboardTarget::Selection,
    }
}

fn damage(damage: tmon::DamageSnapshot) -> TerminalDamageSnapshot {
    match damage {
        tmon::DamageSnapshot::Full => TerminalDamageSnapshot::Full,
        tmon::DamageSnapshot::Partial(spans) => TerminalDamageSnapshot::Partial(
            spans
                .into_iter()
                .map(|span| TerminalDirtySpan {
                    row: span.row,
                    left_col: span.left_col,
                    right_col: span.right_col,
                })
                .collect(),
        ),
    }
}

fn render_damage(
    update: tmon::RenderDamageSnapshot,
    palette_revision: u64,
) -> TerminalRenderDamageSnapshot {
    TerminalRenderDamageSnapshot {
        damage: damage(update.damage),
        scrolls: update
            .scrolls
            .into_iter()
            .map(|scroll| TerminalViewportScroll {
                top: scroll.top,
                bottom: scroll.bottom,
                count: scroll.count,
                direction: match scroll.direction {
                    tmon::ScrollDirection::Up => TerminalViewportScrollDirection::Up,
                    tmon::ScrollDirection::Down => TerminalViewportScrollDirection::Down,
                },
            })
            .collect(),
        generation: update.generation,
        palette_revision,
    }
}

fn terminal_palette(palette: &tmon::Palette) -> TerminalPalette {
    let color = |rgb: tmon::Rgb| TerminalColor {
        r: rgb.r,
        g: rgb.g,
        b: rgb.b,
    };
    TerminalPalette {
        indexed: std::array::from_fn(|index| palette.indexed(index as u8).map(color)),
        foreground: palette.foreground().map(color),
        background: palette.background().map(color),
        cursor: palette.cursor().map(color),
        revision: palette.revision(),
    }
}

fn render_cell(cell: &tmon::Cell, combining: Option<tmon::Combining<'_>>) -> TerminalRenderCell {
    let mut encoded = [0; 4];
    let combining = match combining {
        None => None,
        Some(tmon::Combining::Character(character)) => Some(&*character.encode_utf8(&mut encoded)),
        Some(tmon::Combining::Text(text)) => Some(text),
    };
    TerminalRenderCell {
        text: TerminalRenderText::from_cell_suffix(cell.character, combining),
        foreground: render_color(cell.foreground, true),
        background: render_color(cell.background, false),
        underline_color: cell.underline_color.map(|color| render_color(color, true)),
        bold: cell.attributes.bold(),
        dim: cell.attributes.dim(),
        italic: cell.attributes.italic(),
        underline_style: match cell.attributes.underline_style() {
            tmon::UnderlineStyle::None => TerminalUnderlineStyle::None,
            tmon::UnderlineStyle::Single => TerminalUnderlineStyle::Single,
            tmon::UnderlineStyle::Double => TerminalUnderlineStyle::Double,
            tmon::UnderlineStyle::Curly => TerminalUnderlineStyle::Curly,
            tmon::UnderlineStyle::Dotted => TerminalUnderlineStyle::Dotted,
            tmon::UnderlineStyle::Dashed => TerminalUnderlineStyle::Dashed,
        },
        inverse: cell.attributes.inverse(),
        hidden: cell.attributes.hidden(),
        strikethrough: cell.attributes.strikethrough(),
        hyperlink: cell.has_hyperlink(),
        wide_character_spacer: cell.wide_spacer(),
        leading_wide_character_spacer: cell.leading_wide_spacer(),
        line_wrapped: cell.wrapped(),
    }
}

fn render_color(color: tmon::Color, foreground: bool) -> TerminalRenderColor {
    match color {
        tmon::Color::Default if foreground => TerminalRenderColor::DefaultForeground,
        tmon::Color::Default => TerminalRenderColor::DefaultBackground,
        tmon::Color::Indexed(index) => TerminalRenderColor::Indexed(index),
        tmon::Color::Rgb { r, g, b } => TerminalRenderColor::Rgb(TerminalColor { r, g, b }),
    }
}

fn legacy_cell(
    cell: &tmon::Cell,
    palette: &tmon::Palette,
    query_colors: TerminalQueryColors,
) -> TermyCell {
    let mut foreground = cell.foreground;
    let mut background = cell.background;
    if cell.attributes.inverse() {
        std::mem::swap(&mut foreground, &mut background);
    }
    let uses_terminal_default_bg = matches!(background, tmon::Color::Default);
    if cell.attributes.bold()
        && let tmon::Color::Indexed(index @ 0..=7) = foreground
    {
        foreground = tmon::Color::Indexed(index + 8);
    }
    let mut fg = resolve_color(foreground, true, palette, query_colors);
    if cell.attributes.dim() {
        fg.r /= 2;
        fg.g /= 2;
        fg.b /= 2;
    }
    let wide_character_spacer = cell.wide_spacer() || cell.leading_wide_spacer();
    TermyCell {
        char: cell.character,
        fg,
        bg: resolve_color(background, false, palette, query_colors),
        uses_terminal_default_bg,
        bold: cell.attributes.bold(),
        italic: cell.attributes.italic(),
        underline: cell.attributes.underline(),
        strikethrough: cell.attributes.strikethrough(),
        render_text: !wide_character_spacer
            && !cell.attributes.hidden()
            && cell.character != '\0'
            && !cell.character.is_control(),
        wide_character_spacer,
        line_wrapped: cell.wrapped(),
    }
}

fn legacy_update_cells(
    source: &[tmon::Cell],
    damage: &TerminalDamageSnapshot,
    cols: usize,
    palette: &tmon::Palette,
    query_colors: TerminalQueryColors,
) -> Vec<TermyCell> {
    match damage {
        TerminalDamageSnapshot::Full => source
            .iter()
            .map(|cell| legacy_cell(cell, palette, query_colors))
            .collect(),
        TerminalDamageSnapshot::Partial(spans) => spans
            .iter()
            .flat_map(|span| {
                let start = span.row.saturating_mul(cols).saturating_add(span.left_col);
                let end = span
                    .row
                    .saturating_mul(cols)
                    .saturating_add(span.right_col)
                    .saturating_add(1)
                    .min(source.len());
                source.get(start.min(end)..end).unwrap_or_default()
            })
            .map(|cell| legacy_cell(cell, palette, query_colors))
            .collect(),
    }
}

fn normalize_damage_spans(spans: &mut Vec<TerminalDirtySpan>) {
    spans.sort_unstable_by_key(|span| (span.row, span.left_col, span.right_col));
    let mut merged: Vec<TerminalDirtySpan> = Vec::with_capacity(spans.len());
    for span in spans.drain(..) {
        if let Some(previous) = merged.last_mut()
            && previous.row == span.row
            && span.left_col <= previous.right_col.saturating_add(1)
        {
            previous.right_col = previous.right_col.max(span.right_col);
        } else {
            merged.push(span);
        }
    }
    *spans = merged;
}

fn resolve_color(
    color: tmon::Color,
    foreground: bool,
    palette: &tmon::Palette,
    query_colors: TerminalQueryColors,
) -> TermyColor {
    let color = match color {
        tmon::Color::Default if foreground => {
            palette
                .foreground()
                .map_or(query_colors.foreground, |color| TerminalColor {
                    r: color.r,
                    g: color.g,
                    b: color.b,
                })
        }
        tmon::Color::Default => palette
            .background()
            .map_or(query_colors.background, |color| TerminalColor {
                r: color.r,
                g: color.g,
                b: color.b,
            }),
        tmon::Color::Indexed(index) => palette.indexed(index).map_or_else(
            || query_colors.indexed_color(index),
            |color| TerminalColor {
                r: color.r,
                g: color.g,
                b: color.b,
            },
        ),
        tmon::Color::Rgb { r, g, b } => TerminalColor { r, g, b },
    };
    TermyColor {
        r: color.r,
        g: color.g,
        b: color.b,
        a: 255,
    }
}

fn search_character(cell: &tmon::Cell) -> char {
    let render_text = !cell.wide_spacer()
        && !cell.leading_wide_spacer()
        && !cell.attributes.hidden()
        && cell.character != '\0'
        && !cell.character.is_control();
    if render_text { cell.character } else { ' ' }
}

fn graphics_placement(placement: tmon::GraphicsRenderPlacement) -> KittyGraphicsRenderPlacement {
    KittyGraphicsRenderPlacement {
        placement_serial: placement.placement_serial,
        image_id: placement.image_id,
        placement_id: placement.placement_id,
        png: placement.png,
        image_width: placement.image_width,
        image_height: placement.image_height,
        image_generation: placement.image_generation,
        viewport_row: placement.viewport_row,
        col: placement.col,
        source_x: placement.source_x,
        source_y: placement.source_y,
        source_width: placement.source_width,
        source_height: placement.source_height,
        display_cols: placement.display_cols,
        display_rows: placement.display_rows,
        occupied_cols: placement.occupied_cols,
        occupied_rows: placement.occupied_rows,
        x_offset: placement.x_offset,
        y_offset: placement.y_offset,
        z_index: placement.z_index,
    }
}
