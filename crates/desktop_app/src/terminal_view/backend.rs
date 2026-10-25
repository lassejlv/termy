use super::*;
use std::{
    collections::VecDeque,
    ops::Deref,
    time::{Duration, Instant},
};

/// The terminal-engine facade used by the desktop view. Backend-specific
/// details stay behind this enum so panes do not branch on engine internals.
#[allow(clippy::large_enum_variant)]
pub(super) enum Terminal {
    Tmux(PaneTerminal),
    Native(NativeTerminalInstance),
}

pub(super) struct NativeTerminalInstance {
    // Fields drop in declaration order. `terminal` must drop before `session`:
    // dropping the session closes the host pane, and a terminal still attached
    // at that point reports the closed connection as a lost session.
    pub(super) terminal: Mutex<NativeTerminal>,
    pub(super) session: Option<crate::multiplexer::PaneSession>,
    pub(super) wakeup_id: NativeTerminalWakeupId,
    pub(super) wakeup_route: Arc<Mutex<Option<NativeTerminalWakeupRouter>>>,
    /// What this view wants from a session-host pane's size; `None` for local
    /// terminals (see `HostSizeClaim`).
    pub(super) host_size: Option<Mutex<HostSizeClaim>>,
}

/// The attached desktop wins a size conflict over a session-host pane, but
/// only within limits: two views of one pane (two Termy processes, or a
/// script that keeps running `termy mux resize`) would otherwise resize it
/// back and forth forever.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct HostSizeClaim {
    /// The size this view created, attached or last resized the pane at.
    wanted: TerminalSize,
    /// When this view re-asserted `wanted` after another client's resize.
    reasserts: VecDeque<Instant>,
    /// Out of re-asserts: the view shows the host's size until its own
    /// layout changes the size it wants.
    yielded: bool,
}

impl HostSizeClaim {
    pub(super) const MAX_REASSERTS: usize = 3;
    pub(super) const REASSERT_WINDOW: Duration = Duration::from_secs(10);

    pub(super) fn new(wanted: TerminalSize) -> Self {
        Self {
            wanted,
            reasserts: VecDeque::new(),
            yielded: false,
        }
    }

    /// Record a resize this view sent. `overridden` is whether another
    /// client had resized the pane just before.
    pub(super) fn applied(&mut self, size: TerminalSize, overridden: bool, now: Instant) {
        if size != self.wanted {
            // The view's own layout changed: a new size, a fresh budget.
            *self = Self::new(size);
        } else if overridden {
            self.reasserts.push_back(now);
        }
    }

    /// `true` while another client's resize is waiting to be re-asserted.
    pub(super) fn wants_reassert(&self, overridden: bool) -> bool {
        overridden && !self.yielded
    }

    /// The resize decision for one pane. `stale` is whether the size the
    /// host has, or was last asked for, differs from `next`. `overridden`
    /// is whether another client resized the pane after the host showed
    /// `wanted`: then `next == wanted` is re-asserted at most
    /// `MAX_REASSERTS` times within `REASSERT_WINDOW`, after which the view
    /// keeps the host's size until its own layout changes.
    pub(super) fn needs_resize(
        &mut self,
        next: TerminalSize,
        stale: bool,
        overridden: bool,
        now: Instant,
    ) -> bool {
        if !overridden {
            // The host is back at `wanted`; a later conflict is a new one.
            self.yielded = false;
            return stale;
        }
        if next != self.wanted {
            return stale;
        }
        if !self.yielded {
            while self
                .reasserts
                .front()
                .is_some_and(|at| now.duration_since(*at) > Self::REASSERT_WINDOW)
            {
                self.reasserts.pop_front();
            }
            self.yielded = self.reasserts.len() >= Self::MAX_REASSERTS;
        }
        !self.yielded
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TerminalLineRange {
    pub(super) first_line: i32,
    pub(super) last_line: i32,
    pub(super) columns: usize,
}

pub(super) use termy_core::{TerminalViewportScroll, TerminalViewportScrollDirection};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TerminalRenderDamageSnapshot {
    pub(super) damage: TerminalDamageSnapshot,
    pub(super) scrolls: Vec<TerminalViewportScroll>,
    pub(super) generation: Option<u64>,
    pub(super) palette_revision: Option<u64>,
}

impl TerminalRenderDamageSnapshot {
    pub(super) fn from_core(update: termy_core::TerminalRenderDamageSnapshot) -> Self {
        Self {
            damage: update.damage,
            scrolls: update.scrolls,
            generation: Some(update.generation),
            palette_revision: Some(update.palette_revision),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct TerminalCellRef<'a>(pub(super) &'a termy_core::TerminalRenderCell);

impl<'a> From<&'a termy_core::TerminalRenderCell> for TerminalCellRef<'a> {
    fn from(cell: &'a termy_core::TerminalRenderCell) -> Self {
        Self(cell)
    }
}

impl TerminalCellRef<'_> {
    pub(super) fn character(self) -> char {
        self.0.text.chars().next().unwrap_or('\0')
    }
    pub(super) fn is_wide_spacer(self) -> bool {
        self.0.wide_character_spacer || self.0.leading_wide_character_spacer
    }
    pub(super) fn is_trailing_wide_spacer(self) -> bool {
        self.0.wide_character_spacer
    }
    pub(super) fn is_hidden(self) -> bool {
        self.0.hidden
    }
    pub(super) fn combining(self) -> Option<SharedString> {
        cell_text_suffix(self.0)
            .map(str::to_owned)
            .map(SharedString::from)
    }
    pub(super) fn append_combining_to(self, text: &mut String) {
        if let Some(suffix) = cell_text_suffix(self.0) {
            text.push_str(suffix);
        }
    }
}

fn cell_text_suffix(cell: &termy_core::TerminalRenderCell) -> Option<&str> {
    let text = cell.text.as_str();
    let suffix_start = text.chars().next().map_or(0, char::len_utf8);
    text.get(suffix_start..).filter(|suffix| !suffix.is_empty())
}

pub(super) fn terminal_engine_label(terminal: Option<&Terminal>) -> &'static str {
    match terminal {
        Some(Terminal::Native(terminal)) => terminal
            .lock()
            .map_or("unknown", |terminal| terminal.engine_label()),
        Some(Terminal::Tmux(terminal)) => terminal.engine_label(),
        None => "-",
    }
}

impl Deref for NativeTerminalInstance {
    type Target = Mutex<NativeTerminal>;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

#[derive(Default)]
pub(super) struct ClipboardTextCache {
    // Outer `Option` records whether GPUI has been queried; the inner value is
    // the clipboard result, which can legitimately be empty.
    value: Option<Option<String>>,
    policy: ClipboardReadPolicy,
    // One prompt answers every OSC 52 read in a drain pass, so a burst of
    // requests cannot stack modal dialogs.
    read_permitted: Option<bool>,
}

impl ClipboardTextCache {
    pub(super) fn new(policy: ClipboardReadPolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    /// OSC 52 reads come from untrusted program output (including remote
    /// hosts), so they follow `clipboard_read` instead of always answering.
    pub(super) fn read_permitted(&mut self, ask: impl FnOnce() -> bool) -> bool {
        match self.policy {
            ClipboardReadPolicy::Allow => true,
            ClipboardReadPolicy::Deny => false,
            ClipboardReadPolicy::Ask => *self.read_permitted.get_or_insert_with(ask),
        }
    }

    pub(super) fn get_or_read(&mut self, read: impl FnOnce() -> Option<String>) -> Option<String> {
        if let Some(value) = &self.value {
            return value.clone();
        }

        let value = read();
        self.value = Some(value.clone());
        value
    }
}

pub(super) struct GpuiClipboardReplyHost<'host, 'cx> {
    cx: &'host mut Context<'cx, TerminalView>,
    clipboard_text: &'host mut ClipboardTextCache,
}

impl<'host, 'cx> GpuiClipboardReplyHost<'host, 'cx> {
    pub(super) fn new(
        cx: &'host mut Context<'cx, TerminalView>,
        clipboard_text: &'host mut ClipboardTextCache,
    ) -> Self {
        Self { cx, clipboard_text }
    }
}

impl TerminalReplyHost for GpuiClipboardReplyHost<'_, '_> {
    fn load_clipboard(&mut self, _target: TerminalClipboardTarget) -> Option<String> {
        let permitted = self.clipboard_text.read_permitted(|| {
            !matches!(
                crate::native_sdk::request_clipboard_permission(
                    "Allow clipboard access?",
                    "A terminal application wants to read your clipboard. Set \
                     clipboard_read = allow or deny in the config to stop asking.",
                    false,
                ),
                crate::native_sdk::ClipboardPermission::Deny
            )
        });
        if !permitted {
            // An empty reply lets the requesting program stop waiting.
            return Some(String::new());
        }
        // GPUI exposes a single host clipboard source here, so both OSC 52
        // targets resolve through the same adapter.
        self.clipboard_text
            .get_or_read(|| self.cx.read_from_clipboard().and_then(|item| item.text()))
    }

    fn read_clipboard(
        &mut self,
        request: TerminalClipboardReadRequest,
    ) -> TerminalClipboardReadResult {
        let remember_permission = if request.permission_granted || request.mime_types.is_empty() {
            false
        } else {
            // The name comes from the requesting program, so it is quoted and
            // stripped of controls instead of read as Termy's own text.
            let name = request.name.as_deref().map_or_else(
                || "A terminal application".to_string(),
                |name| {
                    let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
                    format!("A terminal application calling itself \u{201c}{name}\u{201d}")
                },
            );
            let formats: String = request
                .mime_types
                .join(", ")
                .chars()
                .filter(|c| !c.is_control())
                .take(256)
                .collect();
            let message = format!(
                "{name} wants to read {formats} from your {}.",
                clipboard_location_name(request.location)
            );
            match crate::native_sdk::request_clipboard_permission(
                "Allow clipboard access?",
                &message,
                request.can_remember_permission,
            ) {
                crate::native_sdk::ClipboardPermission::Deny => {
                    return TerminalClipboardReadResult::Denied;
                }
                crate::native_sdk::ClipboardPermission::AllowOnce => false,
                crate::native_sdk::ClipboardPermission::AllowAlways => true,
            }
        };

        let available_formats = if request.list_available {
            match available_formats(self.cx, request.location) {
                Ok(formats) => formats,
                Err(result) => return result,
            }
        } else {
            Vec::new()
        };
        let contents = if request.mime_types.is_empty() {
            Vec::new()
        } else {
            match read_formats(self.cx, request.location, &request.mime_types) {
                Ok(contents) => contents,
                Err(result) => return result,
            }
        };
        TerminalClipboardReadResult::Success {
            available_formats,
            contents,
            remember_permission,
        }
    }

    fn write_clipboard(
        &mut self,
        request: TerminalClipboardWriteRequest,
    ) -> TerminalClipboardWriteResult {
        match request.location {
            TerminalClipboardLocation::Clipboard => {
                let contents = request
                    .contents
                    .into_iter()
                    .map(|content| crate::native_sdk::NativeClipboardContent {
                        mime_type: content.mime_type,
                        data: content.data,
                    })
                    .collect();
                match crate::native_sdk::write_clipboard_contents(contents) {
                    Ok(()) => TerminalClipboardWriteResult::Success {
                        remember_permission: false,
                    },
                    Err(error) => native_write_error(error),
                }
            }
            TerminalClipboardLocation::Primary => write_primary(self.cx, request.contents),
        }
    }
}

fn clipboard_location_name(location: TerminalClipboardLocation) -> &'static str {
    match location {
        TerminalClipboardLocation::Clipboard => "clipboard",
        TerminalClipboardLocation::Primary => "primary selection",
    }
}

fn available_formats(
    cx: &mut Context<'_, TerminalView>,
    location: TerminalClipboardLocation,
) -> Result<Vec<String>, TerminalClipboardReadResult> {
    match location {
        TerminalClipboardLocation::Clipboard => {
            crate::native_sdk::available_clipboard_formats().map_err(native_read_error)
        }
        TerminalClipboardLocation::Primary => primary_item(cx)
            .map(|(_, formats)| formats)
            .ok_or(TerminalClipboardReadResult::Unsupported),
    }
}

fn read_formats(
    cx: &mut Context<'_, TerminalView>,
    location: TerminalClipboardLocation,
    mime_types: &[String],
) -> Result<Vec<TerminalClipboardContent>, TerminalClipboardReadResult> {
    match location {
        TerminalClipboardLocation::Clipboard => {
            crate::native_sdk::read_clipboard_formats(mime_types)
                .map(|contents| {
                    contents
                        .into_iter()
                        .map(|content| TerminalClipboardContent {
                            mime_type: content.mime_type,
                            data: content.data,
                        })
                        .collect()
                })
                .map_err(native_read_error)
        }
        TerminalClipboardLocation::Primary => {
            let (item, _) = primary_item(cx).ok_or(TerminalClipboardReadResult::Unsupported)?;
            let item = item.ok_or(TerminalClipboardReadResult::Unsupported)?;
            let mut contents = Vec::new();
            for mime_type in mime_types {
                for entry in item.entries() {
                    match entry {
                        ClipboardEntry::String(text) if mime_type == "text/plain" => {
                            contents.push(TerminalClipboardContent {
                                mime_type: mime_type.clone(),
                                data: text.text().as_bytes().to_vec(),
                            });
                        }
                        ClipboardEntry::Image(image) if image.format.mime_type() == mime_type => {
                            contents.push(TerminalClipboardContent {
                                mime_type: mime_type.clone(),
                                data: image.bytes.clone(),
                            });
                        }
                        ClipboardEntry::ExternalPaths(paths) if mime_type == "text/uri-list" => {
                            let uri_list = paths
                                .paths()
                                .iter()
                                .filter_map(|path| url::Url::from_file_path(path).ok())
                                .map(|url| url.to_string())
                                .collect::<Vec<_>>()
                                .join("\r\n");
                            contents.push(TerminalClipboardContent {
                                mime_type: mime_type.clone(),
                                data: uri_list.into_bytes(),
                            });
                        }
                        _ => {}
                    }
                }
            }
            if contents.is_empty() {
                Err(TerminalClipboardReadResult::Unsupported)
            } else {
                Ok(contents)
            }
        }
    }
}

// `None` means the primary selection is unsupported on this platform, so the
// wrap is only "unnecessary" on Linux.
#[cfg_attr(
    target_os = "linux",
    expect(clippy::unnecessary_wraps, reason = "None signals non-Linux platforms")
)]
fn primary_item(
    cx: &mut Context<'_, TerminalView>,
) -> Option<(Option<ClipboardItem>, Vec<String>)> {
    #[cfg(target_os = "linux")]
    {
        let item = cx.read_from_primary();
        let mut formats = Vec::new();
        if let Some(item) = &item {
            for entry in item.entries() {
                let mime_type = match entry {
                    ClipboardEntry::String(_) => "text/plain",
                    ClipboardEntry::Image(image) => image.format.mime_type(),
                    ClipboardEntry::ExternalPaths(_) => "text/uri-list",
                };
                if !formats.iter().any(|existing| existing == mime_type) {
                    formats.push(mime_type.to_string());
                }
            }
        }
        Some((item, formats))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cx;
        None
    }
}

fn write_primary(
    cx: &mut Context<'_, TerminalView>,
    contents: Vec<TerminalClipboardContent>,
) -> TerminalClipboardWriteResult {
    #[cfg(target_os = "linux")]
    {
        let item = contents.into_iter().find_map(|content| {
            if content.mime_type == "text/plain" {
                String::from_utf8(content.data)
                    .ok()
                    .map(ClipboardItem::new_string)
            } else {
                gpui_kit::ImageFormat::from_mime_type(&content.mime_type).map(|format| {
                    ClipboardItem::new_image(&gpui_kit::Image::from_bytes(format, content.data))
                })
            }
        });
        let Some(item) = item else {
            return TerminalClipboardWriteResult::InvalidData;
        };
        cx.write_to_primary(item);
        TerminalClipboardWriteResult::Success {
            remember_permission: false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cx, contents);
        TerminalClipboardWriteResult::Unsupported
    }
}

fn native_read_error(
    error: crate::native_sdk::NativeClipboardError,
) -> TerminalClipboardReadResult {
    match error {
        crate::native_sdk::NativeClipboardError::Unsupported
        | crate::native_sdk::NativeClipboardError::Unavailable => {
            TerminalClipboardReadResult::Unsupported
        }
        crate::native_sdk::NativeClipboardError::InvalidData => {
            TerminalClipboardReadResult::IoError
        }
        crate::native_sdk::NativeClipboardError::Io(message) => {
            log::warn!("Kitty clipboard read failed: {message}");
            TerminalClipboardReadResult::IoError
        }
    }
}

fn native_write_error(
    error: crate::native_sdk::NativeClipboardError,
) -> TerminalClipboardWriteResult {
    match error {
        crate::native_sdk::NativeClipboardError::Unsupported
        | crate::native_sdk::NativeClipboardError::Unavailable => {
            TerminalClipboardWriteResult::Unsupported
        }
        crate::native_sdk::NativeClipboardError::InvalidData => {
            TerminalClipboardWriteResult::InvalidData
        }
        crate::native_sdk::NativeClipboardError::Io(message) => {
            log::warn!("Kitty clipboard write failed: {message}");
            TerminalClipboardWriteResult::IoError
        }
    }
}

/// Tell the user once every few seconds that typed or pasted input was lost
/// because the program stopped reading it.
pub(super) fn warn_input_dropped() {
    static LAST_WARNING: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let mut last = LAST_WARNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if last.is_some_and(|last| last.elapsed() < std::time::Duration::from_secs(5)) {
        return;
    }
    *last = Some(std::time::Instant::now());
    crate::ui::toast::warning("Input was dropped: the program isn't reading it");
}

#[cfg(test)]
mod tests {
    use super::TerminalCellRef;

    #[test]
    fn core_cells_preserve_native_combining_text_after_convergence() {
        let terminal = termy_core::Terminal::new_display(termy_core::TerminalSize::default(), None);
        terminal.feed_output("e\u{301}".as_bytes());

        let mut observed = false;
        terminal.visit_viewport_cells(|_, _, _, cell| {
            let cell = TerminalCellRef(cell);
            if cell.character() == 'e' {
                observed = true;
                assert_eq!(
                    cell.combining().map(|text| text.to_string()).as_deref(),
                    Some("\u{301}")
                );
                let mut suffix = String::new();
                cell.append_combining_to(&mut suffix);
                assert_eq!(suffix, "\u{301}");
            }
        });
        assert!(observed);
    }
}
