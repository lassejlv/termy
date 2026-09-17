use super::*;

const TEST_BACKEND_ENV: &str = "TERMY_CORE_TEST_BACKEND";
const EXPERIMENTAL_TMON_ENV: &str = "TERMY_EXPERIMENTAL_TMON_ENGINE";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BackendChoice {
    #[default]
    Alacritty,
    Tmon,
}

impl BackendChoice {
    fn native() -> Self {
        if let Some(choice) = Self::from_test_value(env::var_os(TEST_BACKEND_ENV).as_deref()) {
            return choice;
        }
        let requested = env::var_os(EXPERIMENTAL_TMON_ENV);
        let available = crate::tmon::native_pty_available();
        if requested.as_deref() == Some(std::ffi::OsStr::new("1")) && !available {
            log::warn!(
                "TERMY_EXPERIMENTAL_TMON_ENGINE=1 requested, but Tmon's native PTY is unavailable; \
                 falling back to the native Alacritty terminal engine"
            );
        }
        let choice = Self::from_experimental_value(requested.as_deref(), available);
        if choice == Self::Tmon {
            log::info!("using experimental Tmon terminal engine");
        }
        choice
    }

    fn display() -> Self {
        Self::from_test_value(env::var_os(TEST_BACKEND_ENV).as_deref()).unwrap_or(Self::Tmon)
    }

    fn from_test_value(value: Option<&std::ffi::OsStr>) -> Option<Self> {
        match value.and_then(std::ffi::OsStr::to_str) {
            Some("alacritty") => Some(Self::Alacritty),
            Some("tmon") => Some(Self::Tmon),
            _ => None,
        }
    }

    fn from_experimental_value(value: Option<&std::ffi::OsStr>, available: bool) -> Self {
        if value == Some(std::ffi::OsStr::new("1")) && available {
            Self::Tmon
        } else {
            Self::Alacritty
        }
    }
}

pub(super) enum Backend {
    Alacritty(Box<super::alacritty_backend::AlacrittyBackend>),
    Tmon(Box<super::tmon_backend::TmonBackend>),
    Remote(Box<crate::remote::RemoteBackend>),
}

impl Backend {
    pub(super) fn engine_label(&self) -> &'static str {
        match self {
            Self::Alacritty(_) => "alacritty",
            Self::Tmon(_) => "tmon",
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
        match BackendChoice::native() {
            BackendChoice::Alacritty => super::alacritty_backend::AlacrittyBackend::new(
                size,
                configured_working_dir,
                event_wakeup_tx,
                tab_title_shell_integration,
                runtime_config,
                startup_command,
            )
            .map(Box::new)
            .map(Self::Alacritty),
            BackendChoice::Tmon => super::tmon_backend::TmonBackend::new(
                size,
                configured_working_dir,
                event_wakeup_tx,
                tab_title_shell_integration,
                runtime_config,
                startup_command,
            )
            .map(Box::new)
            .map(Self::Tmon),
        }
    }

    pub(super) fn new_with_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        match BackendChoice::native() {
            BackendChoice::Alacritty => {
                super::alacritty_backend::AlacrittyBackend::new_with_wakeup_notifier(
                    size,
                    configured_working_dir,
                    wakeup_notifier,
                    tab_title_shell_integration,
                    runtime_config,
                    startup_command,
                )
                .map(Box::new)
                .map(Self::Alacritty)
            }
            BackendChoice::Tmon => super::tmon_backend::TmonBackend::new_with_wakeup_notifier(
                size,
                configured_working_dir,
                wakeup_notifier,
                tab_title_shell_integration,
                runtime_config,
                startup_command,
            )
            .map(Box::new)
            .map(Self::Tmon),
        }
    }

    pub(super) fn new_with_launch_and_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
    ) -> anyhow::Result<Self> {
        match BackendChoice::native() {
            BackendChoice::Alacritty => {
                super::alacritty_backend::AlacrittyBackend::new_with_launch_and_wakeup_notifier(
                    size,
                    configured_working_dir,
                    wakeup_notifier,
                    tab_title_shell_integration,
                    runtime_config,
                    launch,
                )
                .map(Box::new)
                .map(Self::Alacritty)
            }
            BackendChoice::Tmon => {
                super::tmon_backend::TmonBackend::new_with_launch_and_wakeup_notifier(
                    size,
                    configured_working_dir,
                    wakeup_notifier,
                    tab_title_shell_integration,
                    runtime_config,
                    launch,
                )
                .map(Box::new)
                .map(Self::Tmon)
            }
        }
    }

    pub(super) fn new_display(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
    ) -> Self {
        Self::new_display_with_wakeup_notifier(size, runtime_config, None)
    }

    pub(super) fn new_display_with_wakeup_notifier(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        match BackendChoice::display() {
            BackendChoice::Alacritty => Self::Alacritty(Box::new(
                super::alacritty_backend::AlacrittyBackend::new_display_with_wakeup_notifier(
                    size,
                    runtime_config,
                    wakeup_notifier,
                ),
            )),
            BackendChoice::Tmon => Self::Tmon(Box::new(
                super::tmon_backend::TmonBackend::new_display_with_wakeup_notifier(
                    size,
                    runtime_config,
                    wakeup_notifier,
                ),
            )),
        }
    }

    #[cfg(test)]
    pub(super) fn new_alacritty_display_for_test(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
    ) -> Self {
        Self::Alacritty(Box::new(
            super::alacritty_backend::AlacrittyBackend::new_display(size, runtime_config),
        ))
    }

    pub(super) fn feed_output(&self, bytes: &[u8]) {
        match self {
            Self::Alacritty(backend) => backend.feed_output(bytes),
            Self::Tmon(backend) => backend.feed_output(bytes),
            Self::Remote(backend) => backend.feed_output(bytes),
        }
    }

    pub(super) fn child_pid(&self) -> Option<u32> {
        match self {
            Self::Alacritty(backend) => backend.child_pid(),
            Self::Tmon(backend) => backend.child_pid(),
            Self::Remote(backend) => backend.child_pid(),
        }
    }

    pub(super) fn set_wakeup_enabled(&self, enabled: bool) {
        match self {
            Self::Alacritty(backend) => backend.set_wakeup_enabled(enabled),
            Self::Tmon(backend) => backend.set_wakeup_enabled(enabled),
            Self::Remote(backend) => backend.set_wakeup_enabled(enabled),
        }
    }

    pub(super) fn write(&self, input: &[u8]) {
        match self {
            Self::Alacritty(backend) => backend.write(input),
            Self::Tmon(backend) => backend.write(input),
            Self::Remote(backend) => backend.write(input),
        }
    }

    pub(super) fn write_owned(&self, input: Vec<u8>) {
        match self {
            Self::Alacritty(backend) => backend.write_owned(input),
            Self::Tmon(backend) => backend.write_owned(input),
            Self::Remote(backend) => backend.write_owned(input),
        }
    }

    pub(super) fn hydrate_output(&self, bytes: &[u8]) {
        match self {
            Self::Alacritty(backend) => backend.hydrate_output(bytes),
            Self::Tmon(backend) => backend.hydrate_output(bytes),
            Self::Remote(backend) => backend.hydrate_output(bytes),
        }
    }

    pub(super) fn write_str(&self, input: &str) {
        match self {
            Self::Alacritty(backend) => backend.write_str(input),
            Self::Tmon(backend) => backend.write_str(input),
            Self::Remote(backend) => backend.write_str(input),
        }
    }

    pub(super) fn resize(&mut self, new_size: TerminalSize) {
        match self {
            Self::Alacritty(backend) => backend.resize(new_size),
            Self::Tmon(backend) => backend.resize(new_size),
            Self::Remote(backend) => backend.resize(new_size),
        }
    }

    pub(super) fn nudge_resize(&self) {
        match self {
            Self::Alacritty(backend) => backend.nudge_resize(),
            Self::Tmon(backend) => backend.nudge_resize(),
            Self::Remote(backend) => backend.nudge_resize(),
        }
    }

    pub(super) fn size(&self) -> TerminalSize {
        match self {
            Self::Alacritty(backend) => backend.size(),
            Self::Tmon(backend) => backend.size(),
            Self::Remote(backend) => backend.size(),
        }
    }

    pub(super) fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        match self {
            Self::Alacritty(backend) => backend.kitty_graphics_placements(),
            Self::Tmon(backend) => backend.kitty_graphics_placements(),
            Self::Remote(backend) => backend.kitty_graphics_placements(),
        }
    }

    pub(super) fn kitty_graphics_revision(&self) -> u64 {
        match self {
            Self::Alacritty(backend) => backend.kitty_graphics_revision(),
            Self::Tmon(backend) => backend.kitty_graphics_revision(),
            Self::Remote(backend) => backend.kitty_graphics_revision(),
        }
    }

    pub(super) fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        match self {
            Self::Alacritty(backend) => backend.kitty_graphics_snapshot(),
            Self::Tmon(backend) => backend.kitty_graphics_snapshot(),
            Self::Remote(backend) => backend.kitty_graphics_snapshot(),
        }
    }

    pub(super) fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.kitty_clipboard_paste_events_enabled(),
            Self::Tmon(backend) => backend.kitty_clipboard_paste_events_enabled(),
            Self::Remote(backend) => backend.kitty_clipboard_paste_events_enabled(),
        }
    }

    pub(super) fn send_kitty_clipboard_paste_event(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> bool {
        match self {
            Self::Alacritty(backend) => {
                backend.send_kitty_clipboard_paste_event(location, available_formats)
            }
            Self::Tmon(backend) => {
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
            Self::Alacritty(backend) => backend.drain_events(host),
            Self::Tmon(backend) => backend.drain_events(host),
            Self::Remote(backend) => backend.drain_events(host),
        }
    }

    pub(super) fn set_query_colors(&mut self, query_colors: TerminalQueryColors) {
        match self {
            Self::Alacritty(backend) => backend.set_query_colors(query_colors),
            Self::Tmon(backend) => backend.set_query_colors(query_colors),
            Self::Remote(backend) => backend.set_query_colors(query_colors),
        }
    }

    pub(super) fn palette(&self) -> crate::TerminalPalette {
        match self {
            Self::Alacritty(backend) => backend.palette(),
            Self::Tmon(backend) => backend.palette(),
            Self::Remote(backend) => backend.palette(),
        }
    }

    pub(super) fn snapshot(&self) -> TermyFrame {
        match self {
            Self::Alacritty(backend) => backend.snapshot(),
            Self::Tmon(backend) => backend.snapshot(),
            Self::Remote(backend) => backend.snapshot(),
        }
    }

    pub(super) fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        match self {
            Self::Alacritty(backend) => backend.frame_update(force_full),
            Self::Tmon(backend) => backend.frame_update(force_full),
            Self::Remote(backend) => backend.frame_update(force_full),
        }
    }

    pub(super) fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        match self {
            Self::Alacritty(backend) => backend.take_render_damage_snapshot(),
            Self::Tmon(backend) => backend.take_render_damage_snapshot(),
            Self::Remote(backend) => backend.take_render_damage_snapshot(),
        }
    }

    pub(super) fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        match self {
            Self::Alacritty(backend) => backend.render_read(force_full),
            Self::Tmon(backend) => backend.render_read(force_full),
            Self::Remote(backend) => backend.render_read(force_full),
        }
    }

    pub(super) fn render_read_with_screen(&self, force_full: bool) -> (TerminalRenderRead, bool) {
        match self {
            Self::Alacritty(backend) => backend.render_read_with_screen(force_full),
            Self::Tmon(backend) => backend.render_read_with_screen(force_full),
            Self::Remote(backend) => backend.render_read_with_screen(force_full),
        }
    }

    pub(super) fn visit_viewport_cells(
        &self,
        visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        match self {
            Self::Alacritty(backend) => backend.visit_viewport_cells(visitor),
            Self::Tmon(backend) => backend.visit_viewport_cells(visitor),
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
            Self::Alacritty(backend) => {
                backend.visit_viewport_ranges_at_generation(generation, spans, visitor)
            }
            Self::Tmon(backend) => {
                backend.visit_viewport_ranges_at_generation(generation, spans, visitor)
            }
            Self::Remote(backend) => {
                backend.visit_viewport_ranges_at_generation(generation, spans, visitor)
            }
        }
    }

    pub(super) fn line_bounds(&self) -> (i32, i32) {
        match self {
            Self::Alacritty(backend) => backend.line_bounds(),
            Self::Tmon(backend) => backend.line_bounds(),
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
            Self::Alacritty(backend) => {
                backend.visit_line_cells(requested_first, requested_last, visitor)
            }
            Self::Tmon(backend) => {
                backend.visit_line_cells(requested_first, requested_last, visitor)
            }
            Self::Remote(backend) => {
                backend.visit_line_cells(requested_first, requested_last, visitor)
            }
        }
    }

    pub(super) fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        match self {
            Self::Alacritty(backend) => backend.search(query),
            Self::Tmon(backend) => backend.search(query),
            Self::Remote(backend) => backend.search(query),
        }
    }

    pub(super) fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        match self {
            Self::Alacritty(backend) => backend.search_with_options(query, options),
            Self::Tmon(backend) => backend.search_with_options(query, options),
            Self::Remote(backend) => backend.search_with_options(query, options),
        }
    }

    pub(super) fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        match self {
            Self::Alacritty(backend) => backend.search_shared(query),
            Self::Tmon(backend) => backend.search_shared(query),
            Self::Remote(backend) => backend.search_shared(query),
        }
    }

    pub(super) fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        match self {
            Self::Alacritty(backend) => backend.search_shared_with_options(query, options),
            Self::Tmon(backend) => backend.search_shared_with_options(query, options),
            Self::Remote(backend) => backend.search_shared_with_options(query, options),
        }
    }

    pub(super) fn hyperlink_at(
        &self,
        row: usize,
        col: usize,
    ) -> Option<crate::links::DetectedLink> {
        match self {
            Self::Alacritty(backend) => backend.hyperlink_at(row, col),
            Self::Tmon(backend) => backend.hyperlink_at(row, col),
            Self::Remote(backend) => backend.hyperlink_at(row, col),
        }
    }

    pub(super) fn link_at(
        &self,
        row: usize,
        col: usize,
    ) -> Option<crate::links::DetectedViewportLink> {
        match self {
            Self::Alacritty(backend) => backend.link_at(row, col),
            Self::Tmon(backend) => backend.link_at(row, col),
            Self::Remote(backend) => backend.link_at(row, col),
        }
    }

    pub(super) fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        match self {
            Self::Alacritty(backend) => backend.take_damage_snapshot(),
            Self::Tmon(backend) => backend.take_damage_snapshot(),
            Self::Remote(backend) => backend.take_damage_snapshot(),
        }
    }

    pub(super) fn scroll_display(&self, delta_lines: i32) -> bool {
        match self {
            Self::Alacritty(backend) => backend.scroll_display(delta_lines),
            Self::Tmon(backend) => backend.scroll_display(delta_lines),
            Self::Remote(backend) => backend.scroll_display(delta_lines),
        }
    }

    pub(super) fn scroll_to_bottom(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.scroll_to_bottom(),
            Self::Tmon(backend) => backend.scroll_to_bottom(),
            Self::Remote(backend) => backend.scroll_to_bottom(),
        }
    }

    pub(super) fn clear_scrollback(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.clear_scrollback(),
            Self::Tmon(backend) => backend.clear_scrollback(),
            Self::Remote(backend) => backend.clear_scrollback(),
        }
    }

    pub(super) fn scroll_state(&self) -> (usize, usize) {
        match self {
            Self::Alacritty(backend) => backend.scroll_state(),
            Self::Tmon(backend) => backend.scroll_state(),
            Self::Remote(backend) => backend.scroll_state(),
        }
    }

    pub(super) fn cursor_state(&self) -> Option<TerminalCursorState> {
        match self {
            Self::Alacritty(backend) => backend.cursor_state(),
            Self::Tmon(backend) => backend.cursor_state(),
            Self::Remote(backend) => backend.cursor_state(),
        }
    }

    pub(super) fn cursor_position(&self) -> (usize, usize) {
        match self {
            Self::Alacritty(backend) => backend.cursor_position(),
            Self::Tmon(backend) => backend.cursor_position(),
            Self::Remote(backend) => backend.cursor_position(),
        }
    }

    pub(super) fn has_pending_events(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.has_pending_events(),
            Self::Tmon(backend) => backend.has_pending_events(),
            Self::Remote(backend) => backend.has_pending_events(),
        }
    }

    pub(super) fn set_term_options(&self, options: TerminalOptions) {
        match self {
            Self::Alacritty(backend) => backend.set_term_options(options),
            Self::Tmon(backend) => backend.set_term_options(options),
            Self::Remote(backend) => backend.set_term_options(options),
        }
    }

    pub(super) fn set_scrollback_history(&self, scrollback_history: usize) {
        match self {
            Self::Alacritty(backend) => backend.set_scrollback_history(scrollback_history),
            Self::Tmon(backend) => backend.set_scrollback_history(scrollback_history),
            Self::Remote(backend) => backend.set_scrollback_history(scrollback_history),
        }
    }

    pub(super) fn bracketed_paste_mode(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.bracketed_paste_mode(),
            Self::Tmon(backend) => backend.bracketed_paste_mode(),
            Self::Remote(backend) => backend.bracketed_paste_mode(),
        }
    }

    pub(super) fn mouse_mode(&self) -> TerminalMouseMode {
        match self {
            Self::Alacritty(backend) => backend.mouse_mode(),
            Self::Tmon(backend) => backend.mouse_mode(),
            Self::Remote(backend) => backend.mouse_mode(),
        }
    }

    pub(super) fn keyboard_mode(&self) -> TerminalKeyboardMode {
        match self {
            Self::Alacritty(backend) => backend.keyboard_mode(),
            Self::Tmon(backend) => backend.keyboard_mode(),
            Self::Remote(backend) => backend.keyboard_mode(),
        }
    }

    pub(super) fn alternate_screen_mode(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.alternate_screen_mode(),
            Self::Tmon(backend) => backend.alternate_screen_mode(),
            Self::Remote(backend) => backend.alternate_screen_mode(),
        }
    }

    #[cfg(test)]
    pub(super) fn send_wakeup_for_test(&self) {
        match self {
            Self::Alacritty(backend) => backend.send_wakeup_for_test(),
            Self::Tmon(_) | Self::Remote(_) => panic!("Alacritty wakeup test used a Tmon backend"),
        }
    }

    #[cfg(test)]
    pub(super) fn try_recv_event_for_test(&self) -> Option<RuntimeEvent> {
        match self {
            Self::Alacritty(backend) => backend.try_recv_event_for_test(),
            Self::Tmon(_) | Self::Remote(_) => panic!("Alacritty wakeup test used a Tmon backend"),
        }
    }

    #[cfg(test)]
    pub(super) fn event_queue_is_empty_for_test(&self) -> bool {
        match self {
            Self::Alacritty(backend) => backend.event_queue_is_empty_for_test(),
            Self::Tmon(_) | Self::Remote(_) => panic!("Alacritty wakeup test used a Tmon backend"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_selector_only_accepts_exact_private_values() {
        assert_eq!(BackendChoice::from_test_value(None), None);
        assert_eq!(
            BackendChoice::from_test_value(Some(std::ffi::OsStr::new("alacritty"))),
            Some(BackendChoice::Alacritty)
        );
        assert_eq!(
            BackendChoice::from_test_value(Some(std::ffi::OsStr::new("tmon"))),
            Some(BackendChoice::Tmon)
        );
        assert_eq!(
            BackendChoice::from_test_value(Some(std::ffi::OsStr::new("1"))),
            None
        );
        assert_eq!(
            BackendChoice::from_test_value(Some(std::ffi::OsStr::new("TMON"))),
            None
        );
    }

    #[test]
    fn experimental_native_selector_requires_exact_opt_in_and_availability() {
        assert_eq!(
            BackendChoice::from_experimental_value(Some(std::ffi::OsStr::new("1")), true),
            BackendChoice::Tmon
        );
        for value in [
            None,
            Some(std::ffi::OsStr::new("0")),
            Some(std::ffi::OsStr::new("true")),
        ] {
            assert_eq!(
                BackendChoice::from_experimental_value(value, true),
                BackendChoice::Alacritty
            );
        }
        assert_eq!(
            BackendChoice::from_experimental_value(Some(std::ffi::OsStr::new("1")), false),
            BackendChoice::Alacritty
        );
    }
}
