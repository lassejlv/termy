use super::*;

/// Native and display sessions use the same engine. Remote sessions preserve
/// the public transport boundary without maintaining another emulator.
pub(super) enum Backend {
    Custom(Box<super::custom_backend::CustomBackend>),
    Remote(Box<crate::remote::RemoteBackend>),
}

impl Backend {
    pub(super) fn engine_label(&self) -> &'static str {
        match self {
            Self::Custom(_) => "custom",
            Self::Remote(_) => "multiplexer",
        }
    }

    pub(super) fn new(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        event_wakeup_tx: Option<Sender<()>>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        super::custom_backend::CustomBackend::new(
            size,
            configured_working_dir,
            event_wakeup_tx,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
        .map(Box::new)
        .map(Self::Custom)
    }

    pub(super) fn new_with_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        super::custom_backend::CustomBackend::new_with_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
        .map(Box::new)
        .map(Self::Custom)
    }

    pub(super) fn new_with_launch_and_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
        restored_output: Option<&[u8]>,
    ) -> anyhow::Result<Self> {
        super::custom_backend::CustomBackend::new_with_launch_and_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            launch,
            restored_output,
        )
        .map(Box::new)
        .map(Self::Custom)
    }

    pub(super) fn new_display(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
    ) -> Self {
        Self::Custom(Box::new(super::custom_backend::CustomBackend::new_display(
            size,
            runtime_config,
        )))
    }

    pub(super) fn new_display_with_wakeup_notifier(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        Self::Custom(Box::new(
            super::custom_backend::CustomBackend::new_display_with_wakeup_notifier(
                size,
                runtime_config,
                wakeup_notifier,
            ),
        ))
    }

    #[inline]
    pub(super) fn feed_output(&self, bytes: &[u8]) {
        match self {
            Self::Custom(backend) => backend.feed_output(bytes),
            Self::Remote(backend) => backend.feed_output(bytes),
        }
    }

    pub(super) fn feed_output_with_replies(&self, bytes: &[u8]) -> Vec<u8> {
        match self {
            Self::Custom(backend) => backend.feed_output_with_replies(bytes),
            Self::Remote(backend) => {
                backend.feed_output(bytes);
                Vec::new()
            }
        }
    }

    pub(super) fn set_protocol_replies_enabled(&self, enabled: bool) {
        if let Self::Custom(backend) = self {
            backend.set_protocol_replies_enabled(enabled);
        }
    }

    /// `None` for multiplexer sessions, whose state lives in the server.
    pub(super) fn encode_snapshot(&self) -> Option<Vec<u8>> {
        match self {
            Self::Custom(backend) => Some(backend.encode_snapshot()),
            Self::Remote(_) => None,
        }
    }

    pub(super) fn restore_snapshot(
        &self,
        bytes: &[u8],
    ) -> Option<Result<(), crate::terminal_engine::SnapshotError>> {
        match self {
            Self::Custom(backend) => Some(backend.restore_snapshot(bytes)),
            Self::Remote(_) => None,
        }
    }

    pub(super) fn child_pid(&self) -> Option<u32> {
        match self {
            Self::Custom(backend) => backend.child_pid(),
            Self::Remote(backend) => backend.child_pid(),
        }
    }

    pub(super) fn set_wakeup_enabled(&self, enabled: bool) {
        match self {
            Self::Custom(backend) => backend.set_wakeup_enabled(enabled),
            Self::Remote(backend) => backend.set_wakeup_enabled(enabled),
        }
    }

    pub(super) fn write(&self, input: &[u8]) {
        match self {
            Self::Custom(backend) => backend.write(input),
            Self::Remote(backend) => backend.write(input),
        }
    }

    pub(super) fn write_owned(&self, input: Vec<u8>) {
        match self {
            Self::Custom(backend) => backend.write_owned(input),
            Self::Remote(backend) => backend.write_owned(input),
        }
    }

    pub(super) fn write_mouse_motion(&self, input: Vec<u8>) {
        match self {
            Self::Custom(backend) => backend.write_mouse_motion(input),
            Self::Remote(backend) => backend.write_owned(input),
        }
    }

    pub(super) fn hydrate_output(&self, bytes: &[u8]) {
        match self {
            Self::Custom(backend) => backend.hydrate_output(bytes),
            Self::Remote(backend) => backend.hydrate_output(bytes),
        }
    }

    pub(super) fn resize(&mut self, new_size: TerminalSize) {
        match self {
            Self::Custom(backend) => backend.resize(new_size),
            Self::Remote(backend) => backend.resize(new_size),
        }
    }

    pub(super) fn nudge_resize(&self) {
        match self {
            Self::Custom(backend) => backend.nudge_resize(),
            Self::Remote(backend) => backend.nudge_resize(),
        }
    }

    pub(super) fn size(&self) -> TerminalSize {
        match self {
            Self::Custom(backend) => backend.size(),
            Self::Remote(backend) => backend.size(),
        }
    }

    pub(super) fn host_size_overridden(&self) -> bool {
        match self {
            Self::Custom(_) => false,
            Self::Remote(backend) => backend.host_size_overridden(),
        }
    }

    pub(super) fn requested_size(&self) -> TerminalSize {
        match self {
            Self::Custom(backend) => backend.size(),
            Self::Remote(backend) => backend.requested_size(),
        }
    }

    pub(super) fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        match self {
            Self::Custom(backend) => backend.kitty_graphics_placements(),
            Self::Remote(backend) => backend.kitty_graphics_placements(),
        }
    }

    pub(super) fn kitty_graphics_revision(&self) -> u64 {
        match self {
            Self::Custom(backend) => backend.kitty_graphics_revision(),
            Self::Remote(backend) => backend.kitty_graphics_revision(),
        }
    }

    pub(super) fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        match self {
            Self::Custom(backend) => backend.kitty_graphics_snapshot(),
            Self::Remote(backend) => backend.kitty_graphics_snapshot(),
        }
    }

    pub(super) fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        match self {
            Self::Custom(backend) => backend.kitty_clipboard_paste_events_enabled(),
            Self::Remote(backend) => backend.kitty_clipboard_paste_events_enabled(),
        }
    }

    pub(super) fn kitty_clipboard_paste_notification(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> Option<Vec<u8>> {
        match self {
            Self::Custom(backend) => {
                backend.kitty_clipboard_paste_notification(location, available_formats)
            }
            // The session host owns both the grant state and transport remotely.
            Self::Remote(_) => None,
        }
    }

    pub(super) fn send_kitty_clipboard_paste_event(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> bool {
        match self {
            Self::Custom(backend) => {
                backend.send_kitty_clipboard_paste_event(location, available_formats)
            }
            Self::Remote(backend) => {
                backend.send_kitty_clipboard_paste_event(location, available_formats)
            }
        }
    }

    pub(super) fn drain_events(
        &self,
        host: &mut impl TerminalReplyHost,
    ) -> (Vec<TerminalEvent>, bool) {
        match self {
            Self::Custom(backend) => backend.drain_events(host),
            Self::Remote(backend) => backend.drain_events(host),
        }
    }

    pub(super) fn complete_clipboard_request(
        &self,
        id: u64,
        reply: Option<crate::TerminalClipboardReply>,
    ) {
        match self {
            Self::Custom(backend) => backend.complete_clipboard_request(id, reply),
            // Remote terminals never defer: their host engine does.
            Self::Remote(_) => {}
        }
    }

    pub(super) fn set_query_colors(&mut self, query_colors: TerminalQueryColors) {
        match self {
            Self::Custom(backend) => backend.set_query_colors(query_colors),
            Self::Remote(backend) => backend.set_query_colors(query_colors),
        }
    }

    pub(super) fn palette(&self) -> crate::TerminalPalette {
        match self {
            Self::Custom(backend) => backend.palette(),
            Self::Remote(backend) => backend.palette(),
        }
    }

    pub(super) fn snapshot(&self) -> TermyFrame {
        match self {
            Self::Custom(backend) => backend.snapshot(),
            Self::Remote(backend) => backend.snapshot(),
        }
    }

    pub(super) fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        match self {
            Self::Custom(backend) => backend.frame_update(force_full),
            Self::Remote(backend) => backend.frame_update(force_full),
        }
    }

    pub(super) fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        match self {
            Self::Custom(backend) => backend.take_render_damage_snapshot(),
            Self::Remote(backend) => backend.take_render_damage_snapshot(),
        }
    }

    pub(super) fn with_render_damage<R>(
        &self,
        read: impl FnOnce(TerminalRenderDamageSnapshot, TerminalDamageCells<'_>) -> R,
    ) -> R {
        match self {
            Self::Custom(backend) => backend.with_render_damage(read),
            Self::Remote(backend) => backend.with_render_damage(read),
        }
    }

    pub(super) fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        match self {
            Self::Custom(backend) => backend.render_read(force_full),
            Self::Remote(backend) => backend.render_read(force_full),
        }
    }

    pub(super) fn render_read_with_screen(&self, force_full: bool) -> (TerminalRenderRead, bool) {
        match self {
            Self::Custom(backend) => backend.render_read_with_screen(force_full),
            Self::Remote(backend) => backend.render_read_with_screen(force_full),
        }
    }

    pub(super) fn visit_viewport_cells(
        &self,
        visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        match self {
            Self::Custom(backend) => backend.visit_viewport_cells(visitor),
            Self::Remote(backend) => backend.visit_viewport_cells(visitor),
        }
    }

    pub(super) fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        match self {
            Self::Custom(backend) => {
                backend.visit_viewport_ranges_at_generation(generation, spans, visitor)
            }
            Self::Remote(backend) => {
                backend.visit_viewport_ranges_at_generation(generation, spans, visitor)
            }
        }
    }

    pub(super) fn visit_viewport_cells_locked(
        &self,
        visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        match self {
            Self::Custom(backend) => backend.visit_viewport_cells_locked(visitor),
            Self::Remote(backend) => backend.visit_viewport_cells(visitor),
        }
    }

    pub(super) fn visit_viewport_ranges_locked_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        match self {
            Self::Custom(backend) => {
                backend.visit_viewport_ranges_locked_at_generation(generation, spans, visitor)
            }
            Self::Remote(backend) => {
                backend.visit_viewport_ranges_at_generation(generation, spans, visitor)
            }
        }
    }

    pub(super) fn line_bounds(&self) -> (i32, i32) {
        match self {
            Self::Custom(backend) => backend.line_bounds(),
            Self::Remote(backend) => backend.line_bounds(),
        }
    }

    pub(super) fn visit_line_cells(
        &self,
        requested_first: i32,
        requested_last: i32,
        visitor: impl FnMut((i32, i32, usize), i32, usize, &crate::TerminalRenderCell),
    ) -> (i32, i32, usize) {
        match self {
            Self::Custom(backend) => {
                backend.visit_line_cells(requested_first, requested_last, visitor)
            }
            Self::Remote(backend) => {
                backend.visit_line_cells(requested_first, requested_last, visitor)
            }
        }
    }

    pub(super) fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        match self {
            Self::Custom(backend) => backend.search(query),
            Self::Remote(backend) => backend.search(query),
        }
    }

    pub(super) fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        match self {
            Self::Custom(backend) => backend.search_with_options(query, options),
            Self::Remote(backend) => backend.search_with_options(query, options),
        }
    }

    pub(super) fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        match self {
            Self::Custom(backend) => backend.search_shared(query),
            Self::Remote(backend) => backend.search_shared(query),
        }
    }

    pub(super) fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        match self {
            Self::Custom(backend) => backend.search_shared_with_options(query, options),
            Self::Remote(backend) => backend.search_shared_with_options(query, options),
        }
    }

    pub(super) fn hyperlink_at(
        &self,
        row: usize,
        col: usize,
    ) -> Option<crate::links::DetectedLink> {
        match self {
            Self::Custom(backend) => backend.hyperlink_at(row, col),
            Self::Remote(backend) => backend.hyperlink_at(row, col),
        }
    }

    pub(super) fn link_at(
        &self,
        row: usize,
        col: usize,
    ) -> Option<crate::links::DetectedViewportLink> {
        match self {
            Self::Custom(backend) => backend.link_at(row, col),
            Self::Remote(backend) => backend.link_at(row, col),
        }
    }

    pub(super) fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        match self {
            Self::Custom(backend) => backend.take_damage_snapshot(),
            Self::Remote(backend) => backend.take_damage_snapshot(),
        }
    }

    pub(super) fn scroll_display(&self, delta_lines: i32) -> bool {
        match self {
            Self::Custom(backend) => backend.scroll_display(delta_lines),
            Self::Remote(backend) => backend.scroll_display(delta_lines),
        }
    }

    pub(super) fn scroll_to_bottom(&self) -> bool {
        match self {
            Self::Custom(backend) => backend.scroll_to_bottom(),
            Self::Remote(backend) => backend.scroll_to_bottom(),
        }
    }

    pub(super) fn clear_scrollback(&self) -> bool {
        match self {
            Self::Custom(backend) => backend.clear_scrollback(),
            Self::Remote(backend) => backend.clear_scrollback(),
        }
    }

    pub(super) fn scroll_state(&self) -> (usize, usize) {
        match self {
            Self::Custom(backend) => backend.scroll_state(),
            Self::Remote(backend) => backend.scroll_state(),
        }
    }

    pub(super) fn cursor_state(&self) -> Option<TerminalCursorState> {
        match self {
            Self::Custom(backend) => backend.cursor_state(),
            Self::Remote(backend) => backend.cursor_state(),
        }
    }

    pub(super) fn cursor_position(&self) -> (usize, usize) {
        match self {
            Self::Custom(backend) => backend.cursor_position(),
            Self::Remote(backend) => backend.cursor_position(),
        }
    }

    pub(super) fn has_pending_events(&self) -> bool {
        match self {
            Self::Custom(backend) => backend.has_pending_events(),
            Self::Remote(backend) => backend.has_pending_events(),
        }
    }

    pub(super) fn set_term_options(&self, options: TerminalOptions) {
        match self {
            Self::Custom(backend) => backend.set_term_options(options),
            Self::Remote(backend) => backend.set_term_options(options),
        }
    }

    pub(super) fn set_scrollback_history(&self, scrollback_history: usize) {
        match self {
            Self::Custom(backend) => backend.set_scrollback_history(scrollback_history),
            Self::Remote(backend) => backend.set_scrollback_history(scrollback_history),
        }
    }

    pub(super) fn bracketed_paste_mode(&self) -> bool {
        match self {
            Self::Custom(backend) => backend.bracketed_paste_mode(),
            Self::Remote(backend) => backend.bracketed_paste_mode(),
        }
    }

    pub(super) fn mouse_mode(&self) -> TerminalMouseMode {
        match self {
            Self::Custom(backend) => backend.mouse_mode(),
            Self::Remote(backend) => backend.mouse_mode(),
        }
    }

    pub(super) fn keyboard_mode(&self) -> TerminalKeyboardMode {
        match self {
            Self::Custom(backend) => backend.keyboard_mode(),
            Self::Remote(backend) => backend.keyboard_mode(),
        }
    }

    pub(super) fn alternate_screen_mode(&self) -> bool {
        match self {
            Self::Custom(backend) => backend.alternate_screen_mode(),
            Self::Remote(backend) => backend.alternate_screen_mode(),
        }
    }
}
