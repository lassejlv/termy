use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::terminal_view) enum PendingKeyRelease {
    Consumed,
    Terminal { pane_id: String },
}

fn should_defer_key_down_to_ime(keystroke: &gpui_kit::Keystroke) -> bool {
    let key = keystroke.key.as_str();
    keystroke.key_char.is_some()
        && !keystroke.modifiers.control
        && !keystroke.modifiers.alt
        && !keystroke.modifiers.platform
        && !keystroke.modifiers.function
        && !matches!(
            key,
            "enter" | "tab" | "space" | "backspace" | "escape" | "delete"
        )
}

/// AltGr text on Windows, which arrives as Ctrl+Alt. GPUI flags it with
/// `prefer_character_input` and skips key bindings for it. Leaving it to the
/// platform text path (WM_CHAR) lets Windows compose AltGr dead keys: writing
/// the key here would send the spacing accent and swallow the dead-key state,
/// so the next letter would arrive uncombined.
fn should_defer_altgr_key_down_to_ime(
    keystroke: &gpui_kit::Keystroke,
    prefer_character_input: bool,
) -> bool {
    cfg!(target_os = "windows")
        && prefer_character_input
        && keystroke.modifiers.control
        && keystroke.modifiers.alt
        && !keystroke.modifiers.platform
        && !keystroke.modifiers.function
        && keystroke
            .key_char
            .as_deref()
            .is_some_and(|text| !text.is_empty())
}

/// A dead key starting an xkb compose sequence (Linux). GPUI shows the dead
/// key as marked text first, then sends a KeyDown with no `key_char` whose
/// `key` is only a US-layout guess for the physical key (e.g. `}` or `\`).
/// The composed character arrives later as text, so forwarding this key
/// would type a stray character before it.
fn is_compose_dead_key(keystroke: &gpui_kit::Keystroke, composing: bool) -> bool {
    composing
        && keystroke.key_char.is_none()
        && !keystroke.modifiers.control
        && !keystroke.modifiers.alt
        && !keystroke.modifiers.platform
        && !keystroke.modifiers.function
        // GPUI falls back to the keysym name (`dead_tilde`) when it can't guess.
        && (keystroke.key.chars().count() == 1 || keystroke.key.starts_with("dead_"))
}

/// How pasted and dropped paths are quoted for the shell that reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathQuoteStyle {
    /// POSIX shells, including Git Bash and WSL on Windows: single quotes.
    Posix,
    /// cmd.exe: double quotes, which a Windows path cannot contain.
    Cmd,
    /// PowerShell: single quotes, with each literal quote doubled.
    PowerShell,
}

impl PathQuoteStyle {
    /// The style for a shell program path or name such as
    /// `C:\Windows\System32\cmd.exe` or `pwsh`.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    fn for_shell_program(program: &str) -> Self {
        let name = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(program)
            .to_ascii_lowercase();
        let name = name.strip_suffix(".exe").unwrap_or(name.as_str());
        match name {
            "cmd" => Self::Cmd,
            "powershell" | "pwsh" => Self::PowerShell,
            _ => Self::Posix,
        }
    }
}

/// PowerShell also ends a single-quoted string at typographic single quotes.
fn is_powershell_single_quote(ch: char) -> bool {
    matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}')
}

fn shell_quote_path(path: &Path, style: PathQuoteStyle) -> String {
    let path_str = path.to_string_lossy();
    let mut quoted = String::with_capacity(path_str.len() + 2);
    match style {
        PathQuoteStyle::Posix => {
            quoted.push('\'');
            quoted.push_str(&path_str.replace('\'', "'\\''"));
            quoted.push('\'');
        }
        PathQuoteStyle::Cmd => {
            quoted.push('"');
            quoted.push_str(&path_str);
            quoted.push('"');
        }
        PathQuoteStyle::PowerShell => {
            quoted.push('\'');
            for ch in path_str.chars() {
                if is_powershell_single_quote(ch) {
                    quoted.push(ch);
                }
                quoted.push(ch);
            }
            quoted.push('\'');
        }
    }
    quoted
}

fn shell_quote_paths(paths: &[PathBuf], style: PathQuoteStyle) -> String {
    paths
        .iter()
        .map(|path| shell_quote_path(path, style))
        .collect::<Vec<_>>()
        .join(" ")
}

fn dropped_paths_to_terminal_paste_input(
    paths: &[PathBuf],
    style: PathQuoteStyle,
) -> Option<Vec<u8>> {
    if paths.is_empty() {
        return None;
    }

    let mut text = shell_quote_paths(paths, style);
    text.push(' ');
    Some(text.into_bytes())
}

/// Pasted line breaks become carriage returns, the byte Enter sends, as in
/// xterm and Windows Terminal. A bare `\n` reaches Windows console programs
/// as Ctrl+J, and `\r\n` would add an empty line after each pasted line.
fn normalize_paste_line_endings(input: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !input.contains(&b'\n') {
        return std::borrow::Cow::Borrowed(input);
    }

    let mut normalized = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        match input[index] {
            b'\r' if input.get(index + 1) == Some(&b'\n') => {
                normalized.push(b'\r');
                index += 2;
            }
            b'\n' => {
                normalized.push(b'\r');
                index += 1;
            }
            byte => {
                normalized.push(byte);
                index += 1;
            }
        }
    }
    std::borrow::Cow::Owned(normalized)
}

/// The full image path of a running process, used to find a pane's shell.
#[cfg(target_os = "windows")]
fn windows_process_image_path(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;

    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    result.ok()?;
    Some(String::from_utf16_lossy(&buffer[..len as usize]))
}

fn should_write_drop_to_target(target_is_active: bool, focus_succeeded: bool) -> bool {
    target_is_active || focus_succeeded
}

#[derive(Debug, PartialEq, Eq)]
struct FileDropTarget {
    pane_id: String,
    is_active: bool,
}

fn classify_file_drop_target(
    hit_pane_id: Option<String>,
    active_pane_id: Option<&str>,
    target_is_terminal: bool,
) -> Option<FileDropTarget> {
    let pane_id = hit_pane_id?;
    target_is_terminal.then(|| FileDropTarget {
        is_active: active_pane_id == Some(pane_id.as_str()),
        pane_id,
    })
}

fn image_extension(format: gpui_kit::ImageFormat) -> &'static str {
    match format {
        gpui_kit::ImageFormat::Gif => "gif",
        gpui_kit::ImageFormat::Png => "png",
        gpui_kit::ImageFormat::Jpeg => "jpg",
        gpui_kit::ImageFormat::Webp => "webp",
        gpui_kit::ImageFormat::Bmp => "bmp",
        gpui_kit::ImageFormat::Tiff => "tiff",
        gpui_kit::ImageFormat::Svg => "svg",
        gpui_kit::ImageFormat::Ico => "ico",
        gpui_kit::ImageFormat::Pnm => "pnm",
    }
}

pub(in crate::terminal_view) fn kitty_png_clipboard_item(png: &[u8]) -> ClipboardItem {
    gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png.to_vec()).into()
}

/// Pasted images stay this long, so the program they were pasted into has
/// time to read them; a later paste then deletes them.
const CLIPBOARD_IMAGE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
const CLIPBOARD_IMAGE_PREFIX: &str = "clipboard-image-";

/// A per-user directory: in a shared temp directory other users could read
/// pasted images, or plant files where the paste expects to write them.
fn clipboard_image_cache_dir() -> std::io::Result<PathBuf> {
    #[cfg(test)]
    let base = Some(std::env::temp_dir().join(format!("termy-test-cache-{}", std::process::id())));
    #[cfg(not(test))]
    let base = dirs::cache_dir();
    let base = base.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no user cache directory")
    })?;
    Ok(base.join("termy").join("clipboard-images"))
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700);
        builder.create(dir)?;
        // Also tighten a directory that already existed.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    builder.create(dir)
}

fn remove_expired_clipboard_images(dir: &Path, max_age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(CLIPBOARD_IMAGE_PREFIX)
        {
            continue;
        }
        let expired = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age > max_age));
        if expired {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Write `image` under a fresh random name (created exclusively, owner-only)
/// in the private `dir`, after removing expired earlier pastes.
fn write_clipboard_image_in(dir: &Path, image: &gpui_kit::Image) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    create_private_dir(dir)?;
    remove_expired_clipboard_images(dir, CLIPBOARD_IMAGE_MAX_AGE);
    let mut file = tempfile::Builder::new()
        .prefix(CLIPBOARD_IMAGE_PREFIX)
        .suffix(&format!(".{}", image_extension(image.format())))
        .tempfile_in(dir)?;
    file.write_all(image.bytes())?;
    let (_, path) = file.keep().map_err(|error| error.error)?;
    Ok(path)
}

fn write_clipboard_image_to_temp_file(image: &gpui_kit::Image) -> std::io::Result<PathBuf> {
    write_clipboard_image_in(&clipboard_image_cache_dir()?, image)
}

fn clipboard_item_to_terminal_paste_input(
    item: &ClipboardItem,
    style: PathQuoteStyle,
) -> std::io::Result<Option<Vec<u8>>> {
    clipboard_entries_to_terminal_paste_input(item.entries(), style)
}

fn clipboard_entries_to_terminal_paste_input(
    entries: &[gpui_kit::ClipboardEntry],
    style: PathQuoteStyle,
) -> std::io::Result<Option<Vec<u8>>> {
    // A copied file may also carry a plain-text display name. Paste the file
    // paths when a file list is present. Without one, text and images follow
    // the previous order: text first, then a temporary file for an image.
    let has_files = entries
        .iter()
        .any(|entry| matches!(entry, gpui_kit::ClipboardEntry::ExternalPaths(_)));
    if has_files {
        let paths: Vec<_> = entries
            .iter()
            .filter_map(|entry| match entry {
                gpui_kit::ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
                _ => None,
            })
            .flatten()
            .cloned()
            .collect();
        if !paths.is_empty() {
            return Ok(Some(shell_quote_paths(&paths, style).into_bytes()));
        }
    }

    let mut text = String::new();
    for entry in entries {
        if let gpui_kit::ClipboardEntry::String(value) = entry {
            text.push_str(value.text());
        }
    }
    if !text.is_empty() {
        return Ok(Some(text.into_bytes()));
    }

    let Some(entry) = entries
        .iter()
        .find(|entry| matches!(entry, gpui_kit::ClipboardEntry::Image(_)))
    else {
        return Ok(None);
    };

    match entry {
        gpui_kit::ClipboardEntry::Image(image) => {
            let path = write_clipboard_image_to_temp_file(image)?;
            Ok(Some(shell_quote_path(&path, style).into_bytes()))
        }
        gpui_kit::ClipboardEntry::String(_) | gpui_kit::ClipboardEntry::ExternalPaths(_) => {
            Ok(None)
        }
    }
}

fn synthetic_modifier_keystroke(key: &str, modifiers: gpui_kit::Modifiers) -> gpui_kit::Keystroke {
    gpui_kit::Keystroke {
        modifiers,
        key: key.to_string(),
        key_char: None,
    }
}

fn modifier_transition_events(
    previous: gpui_kit::Modifiers,
    current: gpui_kit::Modifiers,
) -> Vec<(gpui_kit::Keystroke, TerminalKeyEventKind)> {
    // GPUI surfaces pure modifier transitions separately from key presses, so
    // synthesize terminal key events here when enhanced keyboard reporting is active.
    let mut events = Vec::with_capacity(4);

    for (key, was_pressed, is_pressed) in [
        ("control", previous.control, current.control),
        ("alt", previous.alt, current.alt),
        ("shift", previous.shift, current.shift),
        ("super", previous.platform, current.platform),
    ] {
        if was_pressed && !is_pressed {
            events.push((
                synthetic_modifier_keystroke(key, current),
                TerminalKeyEventKind::Release,
            ));
        }
    }

    for (key, was_pressed, is_pressed) in [
        ("control", previous.control, current.control),
        ("alt", previous.alt, current.alt),
        ("shift", previous.shift, current.shift),
        ("super", previous.platform, current.platform),
    ] {
        if !was_pressed && is_pressed {
            events.push((
                synthetic_modifier_keystroke(key, current),
                TerminalKeyEventKind::Press,
            ));
        }
    }

    events
}

fn overlay_owns_terminal_input_state(
    command_palette_open: bool,
    plugin_ui_open: bool,
    search_open: bool,
    renaming_tab: Option<usize>,
    renaming_workspace: Option<usize>,
    release_notes_open: bool,
) -> bool {
    command_palette_open
        || plugin_ui_open
        || search_open
        || renaming_tab.is_some()
        || renaming_workspace.is_some()
        || release_notes_open
}

fn terminal_modifier_transition_events(
    previous: gpui_kit::Modifiers,
    current: gpui_kit::Modifiers,
    overlay_owns_terminal_input: bool,
) -> Vec<(gpui_kit::Keystroke, TerminalKeyEventKind)> {
    if overlay_owns_terminal_input {
        return Vec::new();
    }

    modifier_transition_events(previous, current)
}

fn should_prepare_terminal_input_write(active_pane_id: Option<&str>, pane_id: &str) -> bool {
    active_pane_id == Some(pane_id)
}

fn take_deferred_ime_key_release(
    deferred_ime_key_releases: &mut HashSet<String>,
    key: &str,
) -> bool {
    deferred_ime_key_releases.remove(key)
}

#[derive(Debug, PartialEq, Eq)]
enum PendingKeyReleaseAction {
    Drop,
    ForwardToPane(String),
}

fn take_pending_key_release_action(
    pending_key_releases: &mut HashMap<String, PendingKeyRelease>,
    key: &str,
) -> PendingKeyReleaseAction {
    match pending_key_releases.remove(key) {
        Some(PendingKeyRelease::Consumed) => PendingKeyReleaseAction::Drop,
        Some(PendingKeyRelease::Terminal { pane_id }) => {
            PendingKeyReleaseAction::ForwardToPane(pane_id)
        }
        // The terminal never saw this key's press: a key binding consumed it,
        // or it was held when the window gained focus. Forwarding the release
        // would hand kitty keyboard clients an unpaired release.
        None => PendingKeyReleaseAction::Drop,
    }
}

impl TerminalView {
    fn overlay_owns_terminal_input(&self) -> bool {
        overlay_owns_terminal_input_state(
            self.is_command_palette_open(),
            self.plugin_ui.is_some(),
            self.search_open,
            self.renaming_tab,
            self.renaming_workspace,
            self.release_notes_open(),
        )
    }

    /// How to quote paths pasted into the active pane, chosen by its shell.
    fn active_pane_path_quote_style(&self) -> PathQuoteStyle {
        #[cfg(target_os = "windows")]
        {
            // tmux runs inside WSL, so its panes are POSIX shells.
            if self.runtime_kind() == RuntimeKind::Tmux {
                return PathQuoteStyle::Posix;
            }
            // The pane's own process names the shell, including tabs opened
            // from the shell menu; the configured shell is the fallback.
            let program = self
                .active_terminal()
                .and_then(Terminal::child_pid)
                .and_then(windows_process_image_path)
                .unwrap_or_else(|| self.terminal_runtime.resolved_shell_program());
            if program.trim().is_empty() {
                // Double quotes work in cmd and PowerShell alike.
                return PathQuoteStyle::Cmd;
            }
            PathQuoteStyle::for_shell_program(&program)
        }
        #[cfg(not(target_os = "windows"))]
        {
            PathQuoteStyle::Posix
        }
    }

    fn pane_keyboard_mode(&self, pane_id: &str) -> TerminalKeyboardMode {
        self.pane_terminal_by_id(pane_id)
            .map(Terminal::keyboard_mode)
            .unwrap_or_default()
    }

    fn prompt_shortcuts_enabled_for_pane(&self, pane_id: &str) -> bool {
        !self
            .pane_terminal_by_id(pane_id)
            .is_some_and(|terminal| terminal.alternate_screen_mode())
    }

    pub(in crate::terminal_view) fn send_input_to_pane(&self, pane_id: &str, input: &[u8]) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_send_input_to_pane(pane_id, input),
            RuntimeKind::Native => {
                let Some(terminal) = self.pane_terminal_by_id(pane_id) else {
                    return false;
                };
                terminal.write_input(input);
                true
            }
        }
    }

    fn send_owned_input_to_pane(&self, pane_id: &str, input: Vec<u8>) -> bool {
        match self.runtime_kind() {
            RuntimeKind::Tmux => self.tmux_send_input_to_pane(pane_id, &input),
            RuntimeKind::Native => {
                let Some(terminal) = self.pane_terminal_by_id(pane_id) else {
                    return false;
                };
                terminal.write_input_owned(input);
                true
            }
        }
    }

    pub(in crate::terminal_view) fn send_input_to_active_pane(&self, input: &[u8]) -> bool {
        let Some(pane_id) = self.active_pane_id() else {
            return false;
        };

        self.send_input_to_pane(pane_id, input)
    }

    fn send_owned_input_to_active_pane(&self, input: Vec<u8>) -> bool {
        let Some(pane_id) = self.active_pane_id() else {
            return false;
        };

        self.send_owned_input_to_pane(pane_id, input)
    }

    fn write_terminal_keystroke_to_pane(
        &mut self,
        pane_id: &str,
        keystroke: &gpui_kit::Keystroke,
        event_kind: TerminalKeyEventKind,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(input) = keystroke_to_input(
            keystroke,
            event_kind,
            self.pane_keyboard_mode(pane_id),
            self.prompt_shortcuts_enabled_for_pane(pane_id),
            self.macos_option_as_alt,
        ) else {
            return false;
        };

        self.write_terminal_owned_input_to_pane(pane_id, input, cx);
        true
    }

    fn remember_consumed_key_release(&mut self, key: &str) {
        self.pending_key_releases
            .insert(key.to_string(), PendingKeyRelease::Consumed);
    }

    fn remember_terminal_key_release(&mut self, key: &str, pane_id: String) {
        self.pending_key_releases
            .insert(key.to_string(), PendingKeyRelease::Terminal { pane_id });
    }

    fn write_forwarded_terminal_key_event(
        &mut self,
        keystroke: &gpui_kit::Keystroke,
        event_kind: TerminalKeyEventKind,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane_id) = self.active_pane_id().map(str::to_owned) else {
            return false;
        };

        if !self.write_terminal_keystroke_to_pane(pane_id.as_str(), keystroke, event_kind, cx) {
            return false;
        }

        if matches!(event_kind, TerminalKeyEventKind::Press) {
            self.remember_terminal_key_release(keystroke.key.as_str(), pane_id);
        }

        true
    }

    fn write_terminal_key_release(
        &mut self,
        keystroke: &gpui_kit::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        // Use the pane that received the press so delayed releases do not drift
        // to whatever pane is active when the key is eventually released.
        match take_pending_key_release_action(
            &mut self.pending_key_releases,
            keystroke.key.as_str(),
        ) {
            PendingKeyReleaseAction::Drop => false,
            PendingKeyReleaseAction::ForwardToPane(pane_id) => self
                .write_terminal_keystroke_to_pane(
                    pane_id.as_str(),
                    keystroke,
                    TerminalKeyEventKind::Release,
                    cx,
                ),
        }
    }

    pub(in crate::terminal_view) fn release_forwarded_modifiers(
        &mut self,
        cx: &mut Context<Self>,
    ) -> bool {
        let previous = std::mem::take(&mut self.last_terminal_modifiers);
        let mut wrote_input = false;
        let mut cleared_selection = false;

        for (keystroke, event_kind) in
            modifier_transition_events(previous, gpui_kit::Modifiers::default())
        {
            let wrote = match event_kind {
                TerminalKeyEventKind::Press => {
                    self.write_forwarded_terminal_key_event(&keystroke, event_kind, cx)
                }
                TerminalKeyEventKind::Release => self.write_terminal_key_release(&keystroke, cx),
                TerminalKeyEventKind::Repeat => false,
            };
            if wrote {
                wrote_input = true;
                cleared_selection |= self.clear_selection();
            }
        }

        if cleared_selection {
            cx.notify();
        }

        wrote_input
    }

    fn write_dropped_paths(&mut self, input: &[u8], cx: &mut Context<Self>) {
        let _ = self.close_terminal_context_menu(cx);
        self.write_terminal_paste_input(input, cx);
        cx.notify();
    }

    fn write_dropped_paths_at_position(
        &mut self,
        paths: &[PathBuf],
        position: gpui_kit::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_owns_terminal_input() || paths.is_empty() {
            return;
        }
        let hit_pane_id = self
            .position_to_pane_cell(position, false)
            .map(|(pane_id, _)| pane_id);
        let target_is_terminal = hit_pane_id
            .as_deref()
            .is_some_and(|pane_id| self.pane_terminal_by_id(pane_id).is_some());
        let Some(target) =
            classify_file_drop_target(hit_pane_id, self.active_pane_id(), target_is_terminal)
        else {
            return;
        };

        let focus_succeeded =
            !target.is_active && self.focus_pane_target(target.pane_id.as_str(), cx);
        if !should_write_drop_to_target(target.is_active, focus_succeeded) {
            return;
        }

        // Quote for the target pane's shell, which is active by now.
        let Some(input) =
            dropped_paths_to_terminal_paste_input(paths, self.active_pane_path_quote_style())
        else {
            return;
        };
        self.write_dropped_paths(&input, cx);
    }

    fn maybe_suppress_tab_switch_hint_for_key_down(
        &mut self,
        key: &str,
        modifiers: gpui_kit::Modifiers,
        cx: &mut Context<Self>,
    ) {
        if self.tab_strip.switch_hints.suppress_for_key_down(
            key,
            modifiers,
            self.tab_switch_hints_blocked(),
            Instant::now(),
        ) {
            cx.notify();
        }
    }

    pub(in super::super) fn handle_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .tab_strip
            .switch_hints
            .handle_modifiers_changed(event.modifiers, Instant::now())
        {
            cx.notify();
        }

        let previous = self.last_terminal_modifiers;
        self.last_terminal_modifiers = event.modifiers;
        if !Self::is_link_modifier(event.modifiers) && self.clear_hovered_link() {
            cx.notify();
        }

        let overlay_owns_terminal_input = self.overlay_owns_terminal_input();
        if overlay_owns_terminal_input {
            // Match key-down/up ownership rules: overlays consume modifier
            // transitions too, so terminal-only synth events must stop here.
            return;
        }
        let mut wrote_input = false;
        let mut cleared_selection = false;
        for (keystroke, event_kind) in terminal_modifier_transition_events(
            previous,
            event.modifiers,
            overlay_owns_terminal_input,
        ) {
            let wrote = match event_kind {
                TerminalKeyEventKind::Press => {
                    self.write_forwarded_terminal_key_event(&keystroke, event_kind, cx)
                }
                TerminalKeyEventKind::Release => self.write_terminal_key_release(&keystroke, cx),
                TerminalKeyEventKind::Repeat => false,
            };
            if wrote {
                wrote_input = true;
                cleared_selection |= self.clear_selection();
            }
        }

        if wrote_input || cleared_selection {
            cx.notify();
        }
    }

    fn prepare_terminal_input_write(&mut self, cx: &mut Context<Self>) {
        self.terminal_scroll_accumulator_y = 0.0;
        self.input_scroll_suppress_until =
            Some(Instant::now() + Duration::from_millis(INPUT_SCROLL_SUPPRESS_MS));
        self.scroll_to_bottom(cx);
    }

    pub(in super::super) fn write_terminal_input_to_pane(
        &mut self,
        pane_id: &str,
        input: &[u8],
        cx: &mut Context<Self>,
    ) {
        if input.is_empty() {
            return;
        }

        if should_prepare_terminal_input_write(self.active_pane_id(), pane_id) {
            self.prepare_terminal_input_write(cx);
        }
        if self.send_input_to_pane(pane_id, input) && self.runtime_kind() == RuntimeKind::Tmux {
            self.schedule_tmux_title_refresh();
        }
    }

    fn write_terminal_owned_input_to_pane(
        &mut self,
        pane_id: &str,
        input: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        if input.is_empty() {
            return;
        }

        if should_prepare_terminal_input_write(self.active_pane_id(), pane_id) {
            self.prepare_terminal_input_write(cx);
        }
        if self.send_owned_input_to_pane(pane_id, input) && self.runtime_kind() == RuntimeKind::Tmux
        {
            self.schedule_tmux_title_refresh();
        }
    }

    pub(in super::super) fn write_terminal_input(&mut self, input: &[u8], cx: &mut Context<Self>) {
        if input.is_empty() {
            return;
        }

        let Some(pane_id) = self.active_pane_id().map(str::to_owned) else {
            return;
        };

        self.write_terminal_input_to_pane(pane_id.as_str(), input, cx);
    }

    fn sanitize_bracketed_paste_input(input: &[u8]) -> Option<Vec<u8>> {
        const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
        const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

        let mut sanitized: Option<Vec<u8>> = None;
        let mut index = 0;
        while index < input.len() {
            let remaining = &input[index..];
            let marker_len = if remaining.starts_with(BRACKETED_PASTE_END) {
                Some(BRACKETED_PASTE_END.len())
            } else if remaining.starts_with(BRACKETED_PASTE_START) {
                Some(BRACKETED_PASTE_START.len())
            } else {
                None
            };

            if let Some(marker_len) = marker_len {
                if sanitized.is_none() {
                    let mut buffer = Vec::with_capacity(input.len());
                    buffer.extend_from_slice(&input[..index]);
                    sanitized = Some(buffer);
                }
                index += marker_len;
                continue;
            }

            if let Some(buffer) = sanitized.as_mut() {
                buffer.push(input[index]);
            }
            index += 1;
        }

        sanitized
    }

    fn framed_bracketed_paste_input(input: &[u8]) -> Vec<u8> {
        const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
        const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

        let sanitized = Self::sanitize_bracketed_paste_input(input);
        let payload = sanitized.as_deref().unwrap_or(input);

        // Send one framed payload so start/content/end ordering is atomic and
        // tmux can pick an efficient high-volume path for large pastes.
        let mut framed = Vec::with_capacity(
            BRACKETED_PASTE_START.len() + payload.len() + BRACKETED_PASTE_END.len(),
        );
        framed.extend_from_slice(BRACKETED_PASTE_START);
        framed.extend_from_slice(payload);
        framed.extend_from_slice(BRACKETED_PASTE_END);
        framed
    }

    pub(in super::super) fn write_terminal_paste_input(
        &mut self,
        input: &[u8],
        cx: &mut Context<Self>,
    ) {
        if input.is_empty() {
            return;
        }

        let input = normalize_paste_line_endings(input);
        let input: &[u8] = &input;
        self.prepare_terminal_input_write(cx);
        let bracketed_paste = self
            .active_terminal()
            .is_some_and(|terminal| terminal.bracketed_paste_mode());
        let wrote_input = if bracketed_paste {
            let framed = Self::framed_bracketed_paste_input(input);
            self.send_owned_input_to_active_pane(framed)
        } else {
            self.send_input_to_active_pane(input)
        };

        if wrote_input && self.runtime_kind() == RuntimeKind::Tmux {
            self.schedule_tmux_title_refresh();
        }
    }

    pub(in super::super) fn write_copy_fallback_input(&mut self, _cx: &mut Context<Self>) {
        #[cfg(not(target_os = "macos"))]
        {
            self.write_terminal_input(&[0x03], _cx);
            self.clear_selection();
            _cx.notify();
        }
    }

    pub(in super::super) fn write_paste_fallback_input(&mut self, _cx: &mut Context<Self>) {
        #[cfg(not(target_os = "macos"))]
        {
            self.write_terminal_input(&[0x16], _cx);
            self.clear_selection();
            _cx.notify();
        }
    }

    pub(in super::super) fn execute_input_command_action(
        &mut self,
        action: CommandAction,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.forward_edit_command_to_native_search(action) {
            return true;
        }
        match action {
            CommandAction::Copy => {
                if self.copy_active_inline_input_selection(cx) {
                    return true;
                }
                if self.clear_stale_kitty_image_state() {
                    cx.notify();
                }
                if let Some(image) = self
                    .kitty_image_selection
                    .as_ref()
                    .and_then(|selection| self.current_kitty_image_placement(selection))
                {
                    cx.write_to_clipboard(kitty_png_clipboard_item(image.image.png().as_ref()));
                    crate::ui::toast::success("Copied image");
                    self.notify_overlay(cx);
                    return true;
                }
                if let Some(selected) = self.selected_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(selected));
                    // Windows copies with plain Ctrl+C, as Windows Terminal does.
                    // Dropping the selection lets the next Ctrl+C interrupt.
                    #[cfg(target_os = "windows")]
                    if self.clear_selection() {
                        cx.notify();
                    }
                } else {
                    self.write_copy_fallback_input(cx);
                }
                true
            }
            CommandAction::Paste => {
                if self.paste_clipboard_into_active_inline_input(cx) {
                    return true;
                }
                if self.send_kitty_clipboard_paste_notification(cx) {
                    return true;
                }
                if let Some(item) = cx.read_from_clipboard() {
                    match clipboard_item_to_terminal_paste_input(
                        &item,
                        self.active_pane_path_quote_style(),
                    ) {
                        Ok(Some(input)) => {
                            self.write_terminal_paste_input(&input, cx);
                            self.clear_selection();
                            cx.notify();
                        }
                        Ok(None) => self.write_paste_fallback_input(cx),
                        Err(error) => {
                            crate::ui::toast::error(format!(
                                "Failed to prepare clipboard image for paste: {error}"
                            ));
                        }
                    }
                } else {
                    self.write_paste_fallback_input(cx);
                }
                true
            }
            CommandAction::SelectAll => {
                if self.select_all_in_active_inline_input(cx) {
                    return true;
                }
                if self.select_all_terminal_contents() {
                    cx.notify();
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn send_kitty_clipboard_paste_notification(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(terminal) = self.active_terminal() else {
            return false;
        };
        if !terminal.kitty_clipboard_paste_events_enabled() {
            return false;
        }
        let available_formats = match crate::native_sdk::available_clipboard_formats() {
            Ok(formats) => formats,
            Err(crate::native_sdk::NativeClipboardError::Unavailable) => Vec::new(),
            Err(error) => {
                log::warn!("Kitty clipboard paste notification failed: {error:?}");
                Vec::new()
            }
        };
        let tmux_notification = terminal.kitty_clipboard_paste_notification(&available_formats);
        let sent = if let Some(notification) = tmux_notification {
            self.send_owned_input_to_active_pane(notification)
        } else {
            terminal.send_kitty_clipboard_paste_event(&available_formats)
        };
        if !sent {
            return false;
        }
        self.clear_selection();
        cx.notify();
        true
    }

    pub(in super::super) fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape" && self.cancel_window_tab_drag(cx) {
            cx.stop_propagation();
            return;
        }
        let cursor_was_hidden = !self.cursor_blink_visible;
        self.reset_cursor_blink_phase();
        let _ = self.close_terminal_context_menu(cx);
        let _ = self.close_new_tab_menu(cx);
        let composing = is_compose_dead_key(&event.keystroke, self.ime_marked_text.is_some());
        let defer_to_ime = should_defer_key_down_to_ime(&event.keystroke)
            || should_defer_altgr_key_down_to_ime(&event.keystroke, event.prefer_character_input);
        let inspector_route = if self.overlay_owns_terminal_input() {
            "overlay"
        } else if composing {
            "compose"
        } else if defer_to_ime {
            "ime"
        } else {
            "pty"
        };
        self.record_inspector_key_event(event, inspector_route, cx);
        let key = event.keystroke.key.as_str();
        self.maybe_suppress_tab_switch_hint_for_key_down(key, event.keystroke.modifiers, cx);

        if self.overlay_owns_terminal_input() {
            if self.release_notes_open() {
                if key.eq_ignore_ascii_case("escape") {
                    self.close_release_notes(cx);
                    self.remember_consumed_key_release(key);
                }
                return;
            }

            if self.is_command_palette_open() {
                if self.handle_command_palette_key_down(key, event.keystroke.modifiers, window, cx)
                {
                    self.remember_consumed_key_release(key);
                }
                return;
            }

            if self.search_open {
                if self.handle_search_key_down(key, event.keystroke.modifiers, cx) {
                    self.remember_consumed_key_release(key);
                }
                return;
            }

            match key {
                "enter" => {
                    if self.renaming_workspace.is_some() {
                        self.commit_rename_workspace(cx);
                    } else {
                        self.commit_rename_tab(cx);
                    }
                    self.remember_consumed_key_release(key);
                    return;
                }
                "escape" => {
                    if self.renaming_workspace.is_some() {
                        self.cancel_rename_workspace(cx);
                    } else {
                        self.cancel_rename_tab(cx);
                    }
                    self.remember_consumed_key_release(key);
                    return;
                }
                _ => return,
            }
        }

        self.last_terminal_modifiers = event.keystroke.modifiers;

        // The composed character (or the dead key itself, if composition is
        // cancelled) reaches the PTY through the input handler.
        if composing {
            self.deferred_ime_key_releases.insert(key.to_string());
            return;
        }

        // Printable character input without modifiers is delegated to the
        // platform IME / input handler so that CJK input methods work.
        // Named special keys (enter, tab, space, etc.) and modifier
        // combinations are still handled here via keystroke_to_input.
        if defer_to_ime {
            self.deferred_ime_key_releases.insert(key.to_string());
            // Let the event propagate to the platform IME handler which
            // will call `replace_text_in_range` on our EntityInputHandler.
            return;
        }

        let event_kind = if event.is_held {
            TerminalKeyEventKind::Repeat
        } else {
            TerminalKeyEventKind::Press
        };
        if self.write_forwarded_terminal_key_event(&event.keystroke, event_kind, cx) {
            let selection_changed = self.clear_selection();
            // Stop propagation so the event does not bubble up to the IME input handler.
            cx.stop_propagation();
            if cursor_was_hidden || selection_changed {
                cx.notify();
            }
        }
    }

    pub(in super::super) fn handle_key_up(
        &mut self,
        event: &KeyUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overlay_owns_terminal_input() {
            self.last_terminal_modifiers = event.keystroke.modifiers;
            return;
        }

        self.last_terminal_modifiers = event.keystroke.modifiers;
        // IME-deferred printable keys never wrote a terminal press, so their
        // matching release must be dropped to avoid an unpaired kitty release.
        if take_deferred_ime_key_release(
            &mut self.deferred_ime_key_releases,
            event.keystroke.key.as_str(),
        ) {
            return;
        }
        if self.write_terminal_key_release(&event.keystroke, cx) {
            let selection_changed = self.clear_selection();
            cx.stop_propagation();
            if selection_changed {
                cx.notify();
            }
        }
    }

    pub(in super::super) fn handle_file_drop(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.write_dropped_paths_at_position(paths.paths(), window.mouse_position(), cx);
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn handle_native_file_drop_result(
        &mut self,
        result: super::NativeDropResult,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(drop) => {
                self.write_dropped_paths_at_position(&drop.paths, drop.position, cx);
            }
            Err(error) => {
                crate::ui::toast::error(error.to_string());
            }
        }
    }
}

#[cfg(test)]
#[path = "input/tests.rs"]
mod tests;
