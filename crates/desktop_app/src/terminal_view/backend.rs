use super::*;
use std::ops::Deref;

/// The terminal-engine facade used by the desktop view. Backend-specific
/// details stay behind this enum so panes do not branch on engine internals.
#[allow(clippy::large_enum_variant)]
pub(super) enum Terminal {
    Tmux(PaneTerminal),
    Native(NativeTerminalInstance),
}

pub(super) struct NativeTerminalInstance {
    pub(super) wakeup_id: NativeTerminalWakeupId,
    pub(super) terminal: Mutex<NativeTerminal>,
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

    pub(super) fn from_damage(damage: TerminalDamageSnapshot) -> Self {
        Self {
            damage,
            scrolls: Vec::new(),
            generation: None,
            palette_revision: None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum TerminalCellRef<'a> {
    Tmux(&'a alacritty_terminal::term::cell::Cell),
    Native(&'a termy_core::TerminalRenderCell),
}

impl<'a> From<&'a alacritty_terminal::term::cell::Cell> for TerminalCellRef<'a> {
    fn from(cell: &'a alacritty_terminal::term::cell::Cell) -> Self {
        Self::Tmux(cell)
    }
}

impl<'a> From<&'a termy_core::TerminalRenderCell> for TerminalCellRef<'a> {
    fn from(cell: &'a termy_core::TerminalRenderCell) -> Self {
        Self::Native(cell)
    }
}

impl TerminalCellRef<'_> {
    pub(super) fn character(self) -> char {
        match self {
            Self::Tmux(cell) => cell.c,
            Self::Native(cell) => cell.text.chars().next().unwrap_or('\0'),
        }
    }

    pub(super) fn is_wide_spacer(self) -> bool {
        match self {
            Self::Tmux(cell) => cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            Self::Native(cell) => cell.wide_character_spacer || cell.leading_wide_character_spacer,
        }
    }

    pub(super) fn is_trailing_wide_spacer(self) -> bool {
        match self {
            Self::Tmux(cell) => cell.flags.contains(Flags::WIDE_CHAR_SPACER),
            Self::Native(cell) => cell.wide_character_spacer,
        }
    }

    pub(super) fn is_hidden(self) -> bool {
        match self {
            Self::Tmux(cell) => cell.flags.contains(Flags::HIDDEN),
            Self::Native(cell) => cell.hidden,
        }
    }

    pub(super) fn combining(self) -> Option<SharedString> {
        match self {
            Self::Tmux(_) => None,
            Self::Native(cell) => cell_text_suffix(cell)
                .map(str::to_owned)
                .map(SharedString::from),
        }
    }

    pub(super) fn append_combining_to(self, text: &mut String) {
        match self {
            Self::Tmux(_) => {}
            Self::Native(cell) => {
                if let Some(suffix) = cell_text_suffix(cell) {
                    text.push_str(suffix);
                }
            }
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
        Some(Terminal::Tmux(_)) => "alacritty",
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
}

impl ClipboardTextCache {
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
            let name = request.name.as_deref().unwrap_or("A terminal application");
            let formats = request.mime_types.join(", ");
            let message = format!(
                "{name} wants to read {formats} from your {}.",
                clipboard_location_name(request.location)
            );
            match termy_native_sdk::request_clipboard_permission(
                "Allow clipboard access?",
                &message,
                request.can_remember_permission,
            ) {
                termy_native_sdk::ClipboardPermission::Deny => {
                    return TerminalClipboardReadResult::Denied;
                }
                termy_native_sdk::ClipboardPermission::AllowOnce => false,
                termy_native_sdk::ClipboardPermission::AllowAlways => true,
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
                    .map(|content| termy_native_sdk::NativeClipboardContent {
                        mime_type: content.mime_type,
                        data: content.data,
                    })
                    .collect();
                match termy_native_sdk::write_clipboard_contents(contents) {
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
            termy_native_sdk::available_clipboard_formats().map_err(native_read_error)
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
            termy_native_sdk::read_clipboard_formats(mime_types)
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
                gpui::ImageFormat::from_mime_type(&content.mime_type).map(|format| {
                    ClipboardItem::new_image(&gpui::Image::from_bytes(format, content.data))
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

fn native_read_error(error: termy_native_sdk::NativeClipboardError) -> TerminalClipboardReadResult {
    match error {
        termy_native_sdk::NativeClipboardError::Unsupported
        | termy_native_sdk::NativeClipboardError::Unavailable => {
            TerminalClipboardReadResult::Unsupported
        }
        termy_native_sdk::NativeClipboardError::InvalidData => TerminalClipboardReadResult::IoError,
        termy_native_sdk::NativeClipboardError::Io(message) => {
            log::warn!("Kitty clipboard read failed: {message}");
            TerminalClipboardReadResult::IoError
        }
    }
}

fn native_write_error(
    error: termy_native_sdk::NativeClipboardError,
) -> TerminalClipboardWriteResult {
    match error {
        termy_native_sdk::NativeClipboardError::Unsupported
        | termy_native_sdk::NativeClipboardError::Unavailable => {
            TerminalClipboardWriteResult::Unsupported
        }
        termy_native_sdk::NativeClipboardError::InvalidData => {
            TerminalClipboardWriteResult::InvalidData
        }
        termy_native_sdk::NativeClipboardError::Io(message) => {
            log::warn!("Kitty clipboard write failed: {message}");
            TerminalClipboardWriteResult::IoError
        }
    }
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
            let cell = TerminalCellRef::Native(cell);
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
